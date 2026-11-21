//! The geometry stage of Grainy's develop pipeline, ported from the Python oracle.
//!
//! It runs on the scene-linear frame before tone, and the same functions transform local-adjustment masks
//! (H×W×1 arrays), so every function takes H×W×C `f32` arrays with 1 or 3 channels.
//!
//! | Oracle | Rust |
//! |---|---|
//! | `engine.geometry` | [`geometry`] |
//! | `processing.optical_geometry` | [`optical_geometry`] |
//! | `straighten.*` | [`straighten`] |
//! | `upright.apply`, `upright.estimate` (after LSD) and its helpers | [`upright`] |
//! | `scan_border.*` | [`scan_border`] |
//! | `rasterio.orient_pixels` | [`orient_pixels`] |
//! | `optics._sensor_view`, `lens_shading`, `lens_geometry`, `_modifier` | [`lens`] |
//! | PIL `Image.rotate(angle, BICUBIC)` on `'F'` images | [`pillow::rotate_bicubic`] |
//!
//! Functions that may leave their input untouched return [`Cow`](std::borrow::Cow): `Borrowed` exactly where
//! the oracle returns its input (or, for `geometry(copy=True)`, an identical copy of it).

mod geometry;
pub mod lens;
mod numeric;
mod optical;
mod orient;
pub mod pillow;
pub mod rng;
pub mod scan_border;
pub mod straighten;
pub mod upright;

pub use geometry::{crop_bounds, geometry, UseCrop};
pub use lens::{LensFlags, LensModel, LensRequest, MAP_BUDGET};
pub use optical::optical_geometry;
pub use orient::{flip_left_right, orient_pixels, rot90};

/// Errors of the geometry stage. `Value` and `Alignment` carry the oracle's own (Korean) messages.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    /// The oracle's `ValueError` (bad stored matrix, lens conditions, orientation, unknown mode…).
    #[error("{0}")]
    Value(String),
    /// `upright.AlignmentError`: automatic alignment found no usable evidence.
    #[error("{0}")]
    Alignment(String),
    /// Lens correction is on (`lensfun_enabled` with a profile) but the caller gave no [`LensModel`].
    #[error("lens correction is enabled but no lens model was supplied")]
    LensModelMissing,
    /// The lens model failed or returned coordinates of the wrong shape.
    #[error("lens model: {0}")]
    Lens(String),
    /// An image primitive rejected its input (OpenCV would have raised `cv2.error`).
    #[error(transparent)]
    Imgproc(#[from] grainy_imgproc::Error),
}

/// Result of the geometry stage.
pub type Result<T> = std::result::Result<T, Error>;
