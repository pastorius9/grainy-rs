//! The tone stage of Grainy's develop pipeline, ported from the Python oracle: everything between the geometry
//! result (scene-linear working pixels) and the colour stage.
//!
//! - [`develop_tone`] is `engine._develop_tone`; [`develop_tone_pixels`], [`tone_encoded`], [`tone_base`],
//!   [`tone_local`], [`highlight_rolloff`] and [`rolloff_white`] are the engine functions it calls.
//! - [`response`] is `luma/tone_response.py` (process 3 Highlights/Shadows), [`mapping`] is
//!   `luma/tone_mapping.py` (the DRT), [`hdr`] the parts of `luma/hdr.py` the stage and `develop` reach.
//! - The film look belongs to grainy-film: it is passed in as a [`FilmRender`].

pub mod cv;
pub mod hdr;
pub mod interp;
pub mod mapping;
mod num;
pub mod response;
pub mod srgb;
mod stage;
pub mod v4;

use grainy_core::Image;
use ndarray::ArrayView3;

pub use response::{ToneBase, ToneLocal};
pub use stage::{
    develop_tone, develop_tone_pixels, highlight_rolloff, rolloff_white, tone_base, tone_encoded, tone_local,
    ROLLOFF_WHITE, SHOULDER_KNEE,
};

/// `film_look.render(a, s['film_look'], s['working_space'])` for settings where `film_look.active(s)`: display-
/// linear print pixels of scene-linear pixels (after exposure, white balance and the lens vignette lift). The
/// stage calls it on the bands the oracle uses, and on the reduced frame of `tone_base`.
pub type FilmRender<'a> = &'a (dyn Fn(ArrayView3<'_, f32>) -> Image + Sync);

/// Errors of the tone stage (inputs the oracle would raise on).
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The stage works on RGB pixels.
    #[error("the tone stage needs 3 channels, got {0}")]
    Channels(usize),
    /// `settings['curve']` is not a list of [x, y] number pairs.
    #[error("the point curve is not a list of [x, y] pairs: {0}")]
    Curve(String),
    /// A frame that cannot be reduced or joined (empty).
    #[error("cannot resize or join an empty frame")]
    Resize,
}

/// Result of the tone stage.
pub type Result<T> = std::result::Result<T, Error>;
