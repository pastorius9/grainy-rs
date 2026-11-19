//! Shared types of the Rust Grainy: the settings map, image aliases and row parallelism.
//!
//! Everything here mirrors the Python oracle (`luma/engine.py`, `luma/panel_keys.py`) so that the existing
//! catalog's stored settings are read as they are.

pub mod analysis;
pub mod numpy;
pub mod panel_keys;
pub mod parallel;
pub mod settings;
pub mod timing;

pub use settings::{defaults, effective, normalized, panels, tone_white, white_balance, Panels, Settings};

/// An image in the oracle's numpy layout: height × width × channels (3), `f32`.
pub type Image = ndarray::Array3<f32>;
/// A single-channel plane (a mask, a luminance map): height × width, `f32`.
pub type Plane = ndarray::Array2<f32>;

/// Errors of the shared layer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Stored settings that are not a JSON object.
    #[error("settings must be a JSON object")]
    NotAnObject,
    /// A value of the wrong kind for its key.
    #[error("setting `{key}` has an unexpected value: {found}")]
    BadValue { key: String, found: String },
}

/// Result of the shared layer.
pub type Result<T> = std::result::Result<T, Error>;
