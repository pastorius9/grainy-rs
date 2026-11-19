//! Private copies of the colour conversions the colour stage calls, exact to the oracle's numpy/OpenCV maths.
//!
//! `rgb_to_hsl`/`hsl_to_rgb` (`luma/processing.py`) go through OpenCV 5's float `COLOR_RGB2HLS`/`COLOR_HLS2RGB`.
//! OpenCV converts each row with 8-lane SIMD code and finishes the last `width % 8` pixels of the row with its
//! scalar code; the two differ in the last bit (fused multiply-add, `2 - (max + min)` against `2 - max - min`,
//! another interpolation form), so both are ported and chosen by column, as OpenCV does on an AVX2 machine.
//! `rgb_to_hsv`/`hsv_to_rgb`, `to_linear`/`to_srgb` are `luma/engine.py`'s numpy float32 code.

use grainy_core::{Image, Plane};

/// Rec. 709 luminance weights as the oracle writes them (`np.array([.2126, .7152, .0722], np.float32)`).
pub const LUMA: [f32; 3] = [f(0.2126), f(0.7152), f(0.0722)];

pub use grainy_core::numpy::f;

pub use grainy_core::numpy::maximum as np_max;

pub use grainy_core::numpy::minimum as np_min;

/// OpenCV's SIMD width for float conversions (AVX2: 8 lanes). The last `width % LANES` pixels of each row use the
/// scalar code.
pub const LANES: usize = 8;

pub use grainy_core::numpy::dot3;

pub use grainy_core::numpy::gemm3;

pub use grainy_core::numpy::py_mod;

pub use grainy_core::numpy::py_mod64;

pub use grainy_core::numpy::clip01;

pub use grainy_imgproc::hls::simd_column;

pub use grainy_imgproc::hls::rgb_to_hls_pixel;

pub use grainy_imgproc::hls::hls_to_rgb_pixel;

pub use grainy_color::hsl::rgb_to_hsl_pixel;

pub use grainy_color::hsl::hsl_to_rgb_pixel;

pub use grainy_color::hsl::rgb_to_hsl;

/// Port of luma/processing.py hsl_to_rgb; stage inputs have equal shapes.
pub fn hsl_to_rgb(h: &Plane, s: &Plane, l: &Plane) -> Image {
    grainy_color::hsl_to_rgb(h, s, l).unwrap_or_else(|_| Image::zeros((0, 0, 3)))
}

/// Port of luma/engine.py rgb_to_hsv (tuple adapter).
pub fn rgb_to_hsv_pixel(p: &[f32]) -> (f32, f32, f32) {
    let [r, g, b, ..] = p else {
        return (f32::NAN, f32::NAN, f32::NAN);
    };
    let [h, s, v] = grainy_color::rgb_pixel_to_hsv([*r, *g, *b]);
    (h, s, v)
}

pub use grainy_color::hsv_pixel_to_rgb as hsv_to_rgb_pixel;

pub use grainy_color::rgb_to_hsv;

/// Port of luma/engine.py hsv_to_rgb; stage inputs have equal shapes.
pub fn hsv_to_rgb(h: &Plane, s: &Plane, v: &Plane) -> Image {
    grainy_color::hsv_to_rgb(h, s, v).unwrap_or_else(|_| Image::zeros((0, 0, 3)))
}

pub use grainy_color::srgb_decode as to_linear_value;

pub use grainy_color::srgb_encode as to_srgb_value;

pub use grainy_color::to_linear;

pub use grainy_color::to_srgb;
