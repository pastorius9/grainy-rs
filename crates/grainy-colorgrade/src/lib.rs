//! The colour stage of Grainy's develop: everything `luma/engine.py _develop_color` does to the tone stage's
//! result (saturation, vibrance, the colour mixer, channel and parametric curves, calibration, point colours,
//! colour grading, the camera matrix and the photo filter), and the Auto Color analysis.
//!
//! The maths is the oracle's float32 numpy/OpenCV code, ported operation by operation (see `convert` for the
//! OpenCV HLS details).

pub mod auto_color;
pub mod convert;
pub mod params;
pub mod photo_filter;
pub mod point_color;
pub mod tools;

use grainy_core::{Image, Settings};

pub use params::ColorParams;
pub use point_color::PointHook;
pub use tools::{color_tools, develop_color_mix, mix_hsl, mix_hsv};

/// Errors of the colour stage.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A setting of the wrong shape or kind.
    #[error("setting `{key}` has an unexpected value: {found}")]
    BadValue { key: String, found: String },
    /// A point colour the oracle rejects (its message).
    #[error("{0}")]
    InvalidPoint(&'static str),
    /// An image that is not H×W×3.
    #[error("the colour stage needs an H×W×3 image, got {0:?}")]
    BadImage(Vec<usize>),
}

/// Result of the colour stage.
pub type Result<T> = std::result::Result<T, Error>;

/// Port of `luma/engine.py _develop_color` (CPU path) for normalized settings.
pub fn develop_color(image: &Image, s: &Settings) -> Result<Image> {
    develop_color_with(image, s, None)
}

/// [`develop_color`] with the point-colour picker hook (`on_point_input`).
pub fn develop_color_with(image: &Image, s: &Settings, on_point_input: Option<PointHook<'_>>) -> Result<Image> {
    if image.dim().2 != 3 {
        return Err(Error::BadImage(image.shape().to_vec()));
    }
    let params = ColorParams::from_settings(s)?;
    if params.hdr {
        let mut bounded_settings = s.clone();
        bounded_settings.set("hdr", false);
        return color_operation(image, |bounded| develop_color_with(bounded, &bounded_settings, on_point_input));
    }
    let a = color_tools(&develop_color_mix(image, &params), &params, on_point_input)?;
    Ok(if s.bool("photo_filter_enabled") { photo_filter::apply(&a, s) } else { a })
}

/// Port of `luma/hdr.py color_operation`: runs a bounded colour operation on pixels divided by their per-pixel
/// gain `max(1, max channel)` and multiplies the gain back.
pub fn color_operation<F>(pixels: &Image, operation: F) -> Result<Image>
where
    F: FnOnce(&Image) -> Result<Image>,
{
    let gain = |p: &[f32]| {
        let m = convert::np_max(convert::np_max(p[0], p[1]), p[2]);
        convert::np_max(1.0, m)
    };
    let bounded = tools::per_pixel(pixels, |_, _, p| {
        let g = gain(&p);
        [p[0] / g, p[1] / g, p[2] / g]
    });
    let mut out = operation(&bounded)?;
    ndarray::Zip::from(out.lanes_mut(ndarray::Axis(2))).and(pixels.lanes(ndarray::Axis(2))).par_for_each(|mut o, p| {
        let g = gain(&[p[0], p[1], p[2]]);
        for c in 0..3 {
            o[c] *= g;
        }
    });
    Ok(out)
}
