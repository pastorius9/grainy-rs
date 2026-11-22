//! Ports of `luma/rawcolor.py`, `luma/dcp.py`, `luma/raw_defaults.py` and the
//! LibRaw calls in `luma/engine.py`. Only `sys` contains unsafe Rust.
#![deny(unsafe_code)]

// Retain the workspace's single LittleCMS native library for the C++ shim.
use grainy_cms as _;
// Retain the shared JPEG native library referenced by LibRaw's USE_JPEG path.
use grainy_jpeg_sys as _;

pub mod dcp;
mod math;
pub mod metadata;
pub mod proraw;
pub mod raw_defaults;
pub mod rawcolor;
#[allow(unsafe_code)]
mod sys;
mod tiff;
mod unicode_data;

/// Errors corresponding to the oracle's ValueError, OSError and LibRaw errors.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Port of rawpy.LibRawFileUnsupportedError (`app.import_reason`).
    #[error("Unsupported file format or not RAW file")]
    UnsupportedRaw,
    /// Port of rawpy's remaining decoder errors.
    #[error("LibRaw error {code}: {message}")]
    LibRaw { code: i32, message: String },
    /// Port of validation errors in rawcolor, dcp and raw_defaults.
    #[error("{0}")]
    Invalid(String),
    /// Port of file access errors.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Port of invalid stored JSON records.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Port of illuminants validation errors.
    #[error(transparent)]
    Color(#[from] grainy_color::ColorError),
}
/// Result type for the oracle functions ported by this crate.
pub type Result<T> = std::result::Result<T, Error>;
fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

pub use sys::{extract_thumbnail, libraw_version, DecodeMode, EmbeddedThumb, RawFile, RawMetadata, RawThumbnail};
