//! HSV conversion (`luma/engine.py`: `rgb_to_hsv`, `hsv_to_rgb`). Hue is on [0, 1), all `float32`.
//! Also holds the plane/pixel helpers shared by the HSL and Lab modules.

use grainy_core::{parallel, Image, Plane};
use ndarray::{s, Array3, Zip};

use crate::{ColorError, Result};
use grainy_core::numpy::{f, maximum as np_max, minimum as np_min, py_mod};

/// Port of `luma/engine.py rgb_to_hsv` for one pixel: (hue 0..1, saturation, value).
#[inline]
pub fn rgb_pixel_to_hsv(p: [f32; 3]) -> [f32; 3] {
    let hi = np_max(np_max(p[0], p[1]), p[2]);
    let lo = np_min(np_min(p[0], p[1]), p[2]);
    let d = hi - lo;
    let safe = np_max(d, f(1e-7));
    let mut hue = 0.0f32;
    for (c, base) in [(2usize, 4.0f32), (1, 2.0), (0, 0.0)] {
        let h = ((p[(c + 1) % 3] - p[(c + 2) % 3]) / safe + base) / 6.0;
        if hi == p[c] {
            hue = h;
        }
    }
    let sat = if hi > 0.0 { d / np_max(hi, f(1e-7)) } else { 0.0 };
    [py_mod(hue, 1.0), sat, hi]
}

/// One pixel of `hsv_to_rgb`: `[h, s, v]` to `[r, g, b]`. Hue wraps (negative and above 1 included); `s` and `v`
/// are not clamped.
pub fn hsv_pixel_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h6 = h * 6.0;
    let floor = h6.floor();
    let sector = (floor as i64).rem_euclid(6);
    let f = h6 - floor;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    match sector {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

/// Splits an H×W×3 image into three planes.
pub(crate) fn planes(image: &Image) -> (Plane, Plane, Plane) {
    let take = |c: usize| image.slice(s![.., .., c]).to_owned();
    (take(0), take(1), take(2))
}

/// Stacks three planes into an H×W×3 image.
pub(crate) fn stack(a: &Plane, b: &Plane, c: &Plane) -> Result<Image> {
    if a.dim() != b.dim() || a.dim() != c.dim() {
        return Err(ColorError::ShapeMismatch);
    }
    let (h, w) = a.dim();
    let mut out = Array3::<f32>::zeros((h, w, 3));
    out.slice_mut(s![.., .., 0]).assign(a);
    out.slice_mut(s![.., .., 1]).assign(b);
    out.slice_mut(s![.., .., 2]).assign(c);
    Ok(out)
}

/// Applies `f` to every pixel of `image` (a 3-vector in, a 3-vector out) in parallel row bands.
pub(crate) fn map_pixels(image: &Image, f: impl Fn([f32; 3]) -> [f32; 3] + Sync) -> Image {
    parallel::map_rows(image, |band, mut out| {
        Zip::from(out.rows_mut()).and(band.rows()).for_each(|mut o, i| {
            let v = f([i[0], i[1], i[2]]);
            o[0] = v[0];
            o[1] = v[1];
            o[2] = v[2];
        });
    })
}

/// Port of luma/engine.py `rgb_to_hsv`: returns `(hue, saturation, value)` planes, hue on [0, 1).
pub fn rgb_to_hsv(a: &Image) -> (Plane, Plane, Plane) {
    planes(&map_pixels(a, rgb_pixel_to_hsv))
}

/// Port of luma/engine.py `hsv_to_rgb`; the three planes must have the same shape.
pub fn hsv_to_rgb(h: &Plane, s: &Plane, v: &Plane) -> Result<Image> {
    let hsv = stack(h, s, v)?;
    Ok(map_pixels(&hsv, |p| hsv_pixel_to_rgb(p[0], p[1], p[2])))
}
