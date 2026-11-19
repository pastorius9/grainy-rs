//! Grainy's film looks and film negatives, ported from the Python oracle's `luma/film_look.py` and
//! `luma/negative.py`.
//!
//! Film looks simulate 82 stocks from their makers' datasheet curves (spectral sensitivities, characteristic
//! curves, dye densities; data in `assets/film`, compiled in). A stock is built once and cached.

// The numerical ports index channels and table rows as the oracle does.
#![allow(clippy::needless_range_loop)]

mod catalog;
mod embedded;
pub mod negative;
use grainy_core::numpy::float32 as npmath;
mod numeric;
mod render;
mod spectral;
mod stock;

pub use catalog::{
    active, by_maker, catalog, entry, find, grain_settings, known, short_name, with_film, Entry, Folder, Found, KINDS,
    MAKERS, SPLIT,
};
pub use render::{halation, matrices, prophoto, prophoto_xyz, render, WorkingSpace};
pub use spectral::{
    adaptation, blackbody, colorimetry, d65, daylight, light, sampled, wavelengths, xyz_srgb, Colorimetry, Dyes,
    BRADFORD, GREY, PRINT_GREY, SRGB_XYZ, STATUS_A,
};
pub use stock::{stock, Stock, LN10, NODES, SAMPLES, SPAN, TINY};

/// Errors of film looks and negatives.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A stock id that is not in the catalogue.
    #[error("unknown film stock `{0}`")]
    UnknownStock(String),
    /// A light name `light()` does not know.
    #[error("unknown light `{0}`")]
    UnknownLight(String),
    /// A data file that cannot be used.
    #[error("film data: {0}")]
    BadData(String),
    /// An image of the wrong shape.
    #[error("image shape: {0}")]
    Shape(String),
    /// Something the oracle does that this crate leaves to another crate.
    #[error("not supported here: {0}")]
    Unsupported(String),
}

/// Result of this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[doc(hidden)]
pub mod testing {
    //! Private helpers exposed for the golden tests only.
    use grainy_core::Plane;

    /// numpy's float32 exp.
    pub fn np_exp(x: f32) -> f32 {
        crate::npmath::exp(x)
    }

    /// numpy's float32 log.
    pub fn np_log(x: f32) -> f32 {
        crate::npmath::ln(x)
    }

    /// The OpenCV Gaussian blur port.
    pub fn gaussian_blur(plane: &Plane, sigma: f64) -> Plane {
        crate::render::gaussian_blur(plane, sigma)
    }
}
