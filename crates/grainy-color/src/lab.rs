//! sRGB (D65) <-> CIE Lab as the oracle computes it in `float32` (`luma/processing.py`: `_rgb_to_lab`,
//! `_lab_to_rgb`, `rgb_to_lab`, `lab_to_rgb`). Operation order follows the numpy expressions.

#![allow(clippy::excessive_precision)] // the oracle's float32 constants, digit for digit

use grainy_core::Image;

use crate::hsv::map_pixels;

const THRESHOLD: f32 = 0.008856;
const SIXTEEN_116: f32 = (16.0f64 / 116.0) as f32;

fn lab_f(t: f32) -> f32 {
    if t > THRESHOLD {
        t.cbrt()
    } else {
        7.787 * t + SIXTEEN_116
    }
}

fn lab_inverse(t: f32) -> f32 {
    if t <= 0.20689303 {
        (t - SIXTEEN_116) / 7.787
    } else {
        t * t * t
    }
}

/// One pixel of `_rgb_to_lab`: sRGB-encoded `[r, g, b]` (clipped to [0, 1]) to `[L, a, b]`.
pub fn rgb_pixel_to_lab(p: [f32; 3]) -> [f32; 3] {
    let lin = |v: f32| {
        let c = v.clamp(0.0, 1.0);
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (lin(p[0]), lin(p[1]), lin(p[2]));
    let x = (0.412453 * r + 0.357580 * g + 0.180423 * b) / 0.950456;
    let y = 0.212671 * r + 0.715160 * g + 0.072169 * b;
    let z = (0.019334 * r + 0.119193 * g + 0.950227 * b) / 1.088754;
    let (fx, fy, fz) = (lab_f(x), lab_f(y), lab_f(z));
    let lightness = if y > THRESHOLD { 116.0 * fy - 16.0 } else { 903.3 * y };
    [lightness, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

/// One pixel of `_lab_to_rgb`: `[L, a, b]` to sRGB-encoded `[r, g, b]` clipped to [0, 1].
pub fn lab_pixel_to_rgb(p: [f32; 3]) -> [f32; 3] {
    let (lightness, a, b) = (p[0], p[1], p[2]);
    let low = lightness <= 7.9996248;
    let fy = if low { 7.787 * (lightness / 903.3) + SIXTEEN_116 } else { (lightness + 16.0) / 116.0 };
    let y = if low { lightness / 903.3 } else { fy * fy * fy };
    let x = lab_inverse(fy + a / 500.0);
    let z = lab_inverse(fy - b / 200.0);
    let rgb = [
        3.07993494 * x - 1.53715152 * y - 0.542783419 * z,
        -0.921234183 * x + 1.87599 * y + 0.0452441813 * z,
        0.052889682 * x - 0.204041338 * y + 1.15115166 * z,
    ];
    rgb.map(|v| {
        let v = v.clamp(0.0, 1.0);
        if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        }
    })
}

/// Port of luma/processing.py `rgb_to_lab`: sRGB-encoded image (clipped to [0, 1]) to Lab
/// (L in 0..100, a and b about ±128), `float32`.
pub fn rgb_to_lab(rgb: &Image) -> Image {
    map_pixels(rgb, rgb_pixel_to_lab)
}

/// Port of luma/processing.py `lab_to_rgb`: Lab image to sRGB-encoded, clipped to [0, 1], `float32`.
pub fn lab_to_rgb(lab: &Image) -> Image {
    map_pixels(lab, lab_pixel_to_rgb)
}
