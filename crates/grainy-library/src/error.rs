//! Errors of the library crate. The variants name the Python exception classes the oracle raises, so callers
//! (and the golden tests) can tell a `ValueError` message meant for the user from an `OSError`.

use std::io;

/// Failure of a library operation.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Python `ValueError`: the message is shown to the user as the oracle words it.
    #[error("{0}")]
    Value(String),
    /// Python `TypeError` / `AttributeError` (the oracle would crash on such input).
    #[error("{0}")]
    Type(String),
    /// Python `OSError` family; `message` is worded like Python's `str(error)`.
    #[error("{message}")]
    Os {
        /// Python-style text, e.g. `[Errno 2] No such file or directory: 'x'`.
        message: String,
        /// The Rust error kind.
        kind: io::ErrorKind,
    },
    /// `xml.etree.ElementTree.ParseError`.
    #[error("{0}")]
    Parse(String),
    /// `adobe_develop.DataError` (a sidecar the reader refuses).
    #[error("{0}")]
    Data(String),
    /// Failure of the catalogue layer.
    #[error(transparent)]
    Catalog(#[from] grainy_catalog::Error),
    /// SQLite failure (`sqlite3.Error`).
    #[error(transparent)]
    Sql(#[from] grainy_catalog::rusqlite::Error),
    /// JSON failure (`json.JSONDecodeError` is a `ValueError` in Python).
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// A cancelled job (`catalog_maintenance.Cancelled`, `folder_recovery.SearchIncomplete`).
    #[error("cancelled")]
    Cancelled,
}

/// Result of a library operation.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// `ValueError(message)`.
    pub fn value(message: impl Into<String>) -> Self {
        Error::Value(message.into())
    }
    /// The Python class name the oracle would raise, with every `OSError` subclass shown as `OSError`.
    pub fn python_type(&self) -> &'static str {
        match self {
            Error::Value(_) | Error::Json(_) => "ValueError",
            Error::Type(message) => {
                if message.starts_with("KeyError") {
                    "KeyError"
                } else if message.starts_with("IndexError") {
                    "IndexError"
                } else if message.starts_with("AttributeError") {
                    "AttributeError"
                } else {
                    "TypeError"
                }
            }
            Error::Os { .. } => "OSError",
            Error::Parse(_) => "ParseError",
            Error::Data(_) => "DataError",
            Error::Sql(_) => "sqlite3.Error",
            Error::Cancelled => "Cancelled",
            Error::Catalog(grainy_catalog::Error::Invalid(_)) => "ValueError",
            Error::Catalog(grainy_catalog::Error::Sql(_)) => "sqlite3.Error",
            Error::Catalog(grainy_catalog::Error::Io(_)) => "OSError",
            Error::Catalog(_) => "Exception",
        }
    }
    /// An `OSError` worded like Python for `path` (`errno_style`: the CRT form used by `open()`, otherwise the
    /// Windows form of `os.stat`).
    pub fn os(error: &io::Error, path: &std::path::Path, errno_style: bool) -> Self {
        Error::Os { message: crate::pathx::os_error_text(error, path, errno_style), kind: error.kind() }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        Error::Os { message: crate::pathx::os_error_text(&error, std::path::Path::new(""), false), kind: error.kind() }
    }
}

/// A cooperative cancel flag shared between threads (`threading.Event` in the oracle).
#[derive(Clone, Debug, Default)]
pub struct Cancel(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Cancel {
    /// A fresh flag that is not set.
    pub fn new() -> Self {
        Self::default()
    }
    /// `Event.set()`.
    pub fn set(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    /// `Event.clear()`.
    pub fn clear(&self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    /// `Event.is_set()`.
    pub fn is_set(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}
