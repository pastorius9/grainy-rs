//! Errors of the crate. The oracle raises Python exceptions; the kind says which one, because the oracle's
//! callers catch some kinds and let others through (`DataError` is a `ValueError`).

use std::fmt;

/// The Python exception class the oracle raises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorKind {
    /// `lua_data.DataError` (a `ValueError`).
    Data,
    /// `ValueError` (also `UnicodeError`, `JSONDecodeError`).
    Value,
    /// `TypeError`.
    Type,
    /// `KeyError`.
    Key,
    /// `IndexError`.
    Index,
    /// `AttributeError`.
    Attribute,
    /// `OverflowError`.
    Overflow,
    /// An operating-system error (file access).
    Io,
}

impl ErrorKind {
    /// The oracle's exception class name.
    pub fn python_name(self) -> &'static str {
        match self {
            ErrorKind::Data => "DataError",
            ErrorKind::Value => "ValueError",
            ErrorKind::Type => "TypeError",
            ErrorKind::Key => "KeyError",
            ErrorKind::Index => "IndexError",
            ErrorKind::Attribute => "AttributeError",
            ErrorKind::Overflow => "OverflowError",
            ErrorKind::Io => "OSError",
        }
    }
}

/// An error with the oracle's exception class and message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Which exception the oracle raises.
    pub kind: ErrorKind,
    /// The message (the oracle's Korean text for `DataError` and the messages stored in reports).
    pub message: String,
}

impl Error {
    /// A `DataError`.
    pub fn data(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Data, message: message.into() }
    }
    /// A `ValueError`.
    pub fn value(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Value, message: message.into() }
    }
    /// A `TypeError`.
    pub fn type_error(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Type, message: message.into() }
    }
    /// A `KeyError` (the message is the quoted key, as `str(KeyError)` gives it).
    pub fn key(key: &str) -> Self {
        Self { kind: ErrorKind::Key, message: format!("'{key}'") }
    }
    /// A `KeyError` for a key given by its Python `repr` (`9`, `'zzz'`).
    pub fn key_repr(repr: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Key, message: repr.into() }
    }
    /// An `IndexError`.
    pub fn index(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Index, message: message.into() }
    }
    /// An `AttributeError`.
    pub fn attribute(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Attribute, message: message.into() }
    }
    /// An `OverflowError`.
    pub fn overflow(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Overflow, message: message.into() }
    }
    /// An operating-system error.
    pub fn io(message: impl Into<String>) -> Self {
        Self { kind: ErrorKind::Io, message: message.into() }
    }
    /// Whether the oracle's `except ValueError` clause catches it (`DataError` is a `ValueError`).
    pub fn is_value_error(&self) -> bool {
        matches!(self.kind, ErrorKind::Value | ErrorKind::Data)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}

/// Result of this crate.
pub type Result<T> = std::result::Result<T, Error>;
