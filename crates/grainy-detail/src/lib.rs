//! Grainy's detail stage, ported from the Python Grainy: `luma/engine.py` `_develop_detail` and the detail tools
//! of `luma/processing.py` (noise reduction, dehaze, texture, defringe, clarity, sharpening), ending with the
//! post-crop vignette and grain of `grainy-effects`.
//!
//! The OpenCV filters these tools call are ported privately (`cv`), with the arithmetic of the oracle's
//! opencv-python build, so results are bit-identical to the oracle's CPU path except where its Intel IPP bilateral
//! filter is used (luminance noise reduction of `detail_version` 1 and 2: a few float32 ulps of L*).

pub mod ai_denoise;
mod cv;
pub mod laplacian;
pub mod noise;
mod np;
mod stage;
pub mod tools;

pub use noise::{
    atrous_smooth, box_filter, chroma_guided, chroma_guided_params, luma_noise_reduction, luma_wavelet, noise_profile,
    wavelet_kernel, wavelet_lambda, wavelet_profiles,
};
pub use stage::develop_detail;
pub use tools::{blur, detail_tools, sharpen};

/// Errors of the detail stage.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An image that is not height × width × 3.
    #[error("the detail stage needs an H×W×3 image, got shape {0:?}")]
    Shape(Vec<usize>),
}

/// Result of the detail stage.
pub type Result<T> = std::result::Result<T, Error>;
