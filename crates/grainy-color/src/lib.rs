//! Colour maths of the Rust Grainy, ported from the Python oracle (`luma/engine.py`, `luma/processing.py`,
//! `luma/illuminants.py`).
//!
//! ICC profiles and output conversion are owned by grainy-cms.

pub mod hsl;
mod hsv;
pub mod illuminants;
mod lab;
pub mod primaries;
mod transfer;

pub use grainy_core::settings::kelvin_rgb;
pub use grainy_core::white_balance;
pub use hsl::{hls_pixel_to_rgb, hsl_to_rgb, rgb_pixel_to_hls, rgb_to_hsl};
pub use hsv::{hsv_pixel_to_rgb, hsv_to_rgb, rgb_pixel_to_hsv, rgb_to_hsv};
pub use lab::{lab_pixel_to_rgb, lab_to_rgb, rgb_pixel_to_lab, rgb_to_lab};
pub use primaries::prophoto_xyz;
pub use transfer::{clip01, clip01_in_place, srgb_decode, srgb_encode, to_linear, to_srgb};

/// Errors of the colour layer (the oracle raises `ValueError` for the illuminant ones).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ColorError {
    /// An illuminant, chromaticity or custom-illuminant payload that the oracle rejects.
    #[error("invalid illuminant data: {0}")]
    Illuminant(&'static str),
    /// Planes of different sizes were combined.
    #[error("planes differ in shape")]
    ShapeMismatch,
}

/// Result of the colour layer.
pub type Result<T> = std::result::Result<T, ColorError>;
