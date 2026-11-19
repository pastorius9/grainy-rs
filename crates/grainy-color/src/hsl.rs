//! Port of luma/processing.py HSL conversions using the exact OpenCV implementation.
use grainy_core::numpy::{clip01, py_mod};
use grainy_core::{Image, Plane};
pub use grainy_imgproc::hls::{hls_to_rgb_pixel, rgb_to_hls_pixel, simd_column};
use ndarray::{Array2, Axis};
/// Port of `luma/processing.py rgb_to_hsl` for one pixel: hue 0..1, saturation, lightness of the clipped pixel.
#[inline]
pub fn rgb_to_hsl_pixel(p: &[f32], simd: bool) -> (f32, f32, f32) {
    let (h, l, s) = rgb_to_hls_pixel(clip01(p[0]), clip01(p[1]), clip01(p[2]), simd);
    (h / 360.0, s, l)
}

/// Port of `luma/processing.py hsl_to_rgb` for one pixel (hue in turns, wrapped; s and l clipped).
#[inline]
pub fn hsl_to_rgb_pixel(h: f32, s: f32, l: f32, simd: bool) -> [f32; 3] {
    hls_to_rgb_pixel(py_mod(h, 1.0) * 360.0, clip01(l), clip01(s), simd)
}

/// Port of `luma/processing.py rgb_to_hsl`: (hue 0..1, saturation, lightness) planes.
pub fn rgb_to_hsl(image: &Image) -> (Plane, Plane, Plane) {
    let (height, width, _) = image.dim();
    let mut h = Array2::<f32>::zeros((height, width));
    let mut s = h.clone();
    let mut l = h.clone();
    for (y, row) in image.axis_iter(Axis(0)).enumerate() {
        for (x, p) in row.axis_iter(Axis(0)).enumerate() {
            let p = [p[0], p[1], p[2]];
            let (hh, ss, ll) = rgb_to_hsl_pixel(&p, simd_column(x, width));
            h[[y, x]] = hh;
            s[[y, x]] = ss;
            l[[y, x]] = ll;
        }
    }
    (h, s, l)
}

/// Port of `luma/processing.py hsl_to_rgb`.
pub fn hsl_to_rgb(h: &Plane, s: &Plane, l: &Plane) -> crate::Result<Image> {
    crate::hsv::stack(h, l, s)?;
    let (height, width) = h.dim();
    let mut out = Image::zeros((height, width, 3));
    for y in 0..height {
        for x in 0..width {
            let rgb = hsl_to_rgb_pixel(h[[y, x]], s[[y, x]], l[[y, x]], simd_column(x, width));
            for c in 0..3 {
                out[[y, x, c]] = rgb[c];
            }
        }
    }
    Ok(out)
}

/// Port of cv2.COLOR_RGB2HLS for a scalar pixel.
pub fn rgb_pixel_to_hls(p: [f32; 3]) -> [f32; 3] {
    let (h, l, s) = rgb_to_hls_pixel(p[0], p[1], p[2], false);
    [h, l, s]
}
/// Port of cv2.COLOR_HLS2RGB for a scalar pixel.
pub fn hls_pixel_to_rgb(h: f32, l: f32, s: f32) -> [f32; 3] {
    hls_to_rgb_pixel(h, l, s, false)
}
