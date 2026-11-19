//! sRGB transfer curves and clipping (`luma/engine.py`: `to_linear`, `to_srgb`, `_to_srgb`, `_clip01`).

use grainy_core::numpy::{f, maximum as np_max};
use grainy_core::{parallel, Image};

fn map_values(a: &Image, f: impl Fn(f32) -> f32 + Sync) -> Image {
    parallel::map_rows(a, |band, mut out| out.zip_mut_with(&band, |o, i| *o = f(*i)))
}

/// Port of `luma/engine.py to_linear` for one value (float32).
#[inline]
pub fn srgb_decode(a: f32) -> f32 {
    if a <= f(0.04045) {
        a / f(12.92)
    } else {
        (np_max(a + f(0.055), 0.0) / f(1.055)).powf(f(2.4))
    }
}

/// Port of `luma/engine.py _to_srgb` for one value (float32).
#[inline]
pub fn srgb_encode(rgb: f32) -> f32 {
    let a = np_max(rgb, 0.0);
    if a <= f(0.0031308) {
        a * f(12.92)
    } else {
        f(1.055) * a.powf(f(1.0 / 2.4)) - f(0.055)
    }
}

/// Port of luma/engine.py `to_linear`: sRGB-encoded image to scene-linear (`float32`, no clipping above 1).
pub fn to_linear(rgb: &Image) -> Image {
    map_values(rgb, srgb_decode)
}

/// Port of luma/engine.py `to_srgb` (`_to_srgb` on row bands): scene-linear to sRGB-encoded. Negative values
/// become 0, values above 1 are encoded above 1.
pub fn to_srgb(rgb: &Image) -> Image {
    map_values(rgb, srgb_encode)
}

/// Port of luma/engine.py `_clip01` for a caller that does not own the array: a clipped copy.
pub fn clip01(a: &Image) -> Image {
    map_values(a, |v| v.clamp(0.0, 1.0))
}

/// Port of luma/engine.py `_clip01` with `owned=True`: clips in place (NaN stays NaN, as `np.clip`).
pub fn clip01_in_place(a: &mut Image) {
    parallel::for_each_pixel(a, |p| p.iter_mut().for_each(|v| *v = v.clamp(0.0, 1.0)));
}
