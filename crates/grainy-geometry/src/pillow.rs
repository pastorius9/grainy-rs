//! Pillow's `Image.rotate(angle, Image.Resampling.BICUBIC, expand=False)` on float32 (`'F'` mode) images, as
//! the oracle's CPU path straightens each channel (Pillow 12.3.0: `Image.rotate`, `Image.transform` and
//! `src/libImaging/Geometry.c`: `ImagingGenericTransform`, `affine_transform`, `bicubic_filter32F`).
//! Pillow is under the HPND licence; see `NOTICE`.

use ndarray::{Array3, ArrayView3};
use rayon::prelude::*;

use crate::numeric::{py_mod, py_round_digits};
use crate::orient::rot90;

/// The affine matrix `Image.rotate` passes to `Image.transform` for a turn about the centre of a
/// `width`×`height` image (`angle` in degrees counter-clockwise, already reduced modulo 360).
pub fn rotate_matrix(angle: f64, width: usize, height: usize) -> [f64; 6] {
    let (cx, cy) = (width as f64 / 2.0, height as f64 / 2.0);
    let r = -angle.to_radians();
    let mut m = [
        py_round_digits(r.cos(), 15),
        py_round_digits(r.sin(), 15),
        0.0,
        py_round_digits(-r.sin(), 15),
        py_round_digits(r.cos(), 15),
        0.0,
    ];
    // transform(-cx - 0, -cy - 0, matrix): a*x + b*y + c
    let (x, y) = (-cx, -cy);
    // a*x + b*y + c with c = 0.0 (IEEE addition commutes).
    m[2] += m[0] * x + m[1] * y;
    m[5] += m[3] * x + m[4] * y;
    m[2] += cx;
    m[5] += cy;
    m
}

/// `#define FLOOR(v)` of Geometry.c.
fn floor_int(v: f64) -> i64 {
    if v < 0.0 {
        v.floor() as i64
    } else {
        v as i64
    }
}

/// The first `BICUBIC` pass of `bicubic_filter32F`: its arguments are `FLOAT32` lvalues, so the four
/// coefficients are evaluated in float32 arithmetic before the double-precision Horner step.
fn cubic_f32(v1: f32, v2: f32, v3: f32, v4: f32, d: f64) -> f64 {
    let p1 = f64::from(v2);
    let p2 = f64::from(-v1 + v3);
    let p3 = f64::from(2.0f32 * (v1 - v2) + v3 - v4);
    let p4 = f64::from(-v1 + v2 - v3 + v4);
    p1 + d * (p2 + d * (p3 + d * p4))
}

/// The second `BICUBIC` pass, on doubles.
fn cubic_f64(v1: f64, v2: f64, v3: f64, v4: f64, d: f64) -> f64 {
    let p1 = v2;
    let p2 = -v1 + v3;
    let p3 = 2.0 * (v1 - v2) + v3 - v4;
    let p4 = -v1 + v2 - v3 + v4;
    p1 + d * (p2 + d * (p3 + d * p4))
}

/// `bicubic_filter32F` at source position (`xin`, `yin`) for every channel; `false` outside the image (the
/// caller then writes 0, as `fill` does).
fn bicubic(src: &[f32], width: usize, height: usize, channels: usize, xin: f64, yin: f64, out: &mut [f32]) -> bool {
    let (w, h) = (width as i64, height as i64);
    if xin < 0.0 || xin >= width as f64 || yin < 0.0 || yin >= height as f64 {
        return false;
    }
    let xin = xin - 0.5;
    let yin = yin - 0.5;
    let x = floor_int(xin);
    let y = floor_int(yin);
    let dx = xin - x as f64;
    let dy = yin - y as f64;
    let x = x - 1;
    let y = y - 1;
    let clip_x = |v: i64| v.clamp(0, w - 1) as usize;
    let xs = [clip_x(x), clip_x(x + 1), clip_x(x + 2), clip_x(x + 3)];
    let row = |r: usize, c: usize| -> f64 {
        let base = r * width;
        let at = |i: usize| src[(base + xs[i]) * channels + c];
        cubic_f32(at(0), at(1), at(2), at(3), dx)
    };
    for (c, slot) in out.iter_mut().enumerate().take(channels) {
        let v1 = row(y.clamp(0, h - 1) as usize, c);
        let v2 = if y + 1 >= 0 && y + 1 < h { row((y + 1) as usize, c) } else { v1 };
        let v3 = if y + 2 >= 0 && y + 2 < h { row((y + 2) as usize, c) } else { v2 };
        let v4 = if y + 3 >= 0 && y + 3 < h { row((y + 3) as usize, c) } else { v3 };
        *slot = cubic_f64(v1, v2, v3, v4, dy) as f32;
    }
    true
}

/// Port of PIL `Image.rotate(angle, Image.Resampling.BICUBIC, expand=False)` applied to each channel of an
/// H×W×C float32 array (`Image.fromarray(a[..., c])`, mode `'F'`), as `engine.geometry` straightens with
/// `angle = -straighten`. Pillow's exact fast paths are kept: 0° copies, 180° and (square images only)
/// 90°/270° transpose. Pixels whose source falls outside the image are 0.
pub fn rotate_bicubic(a: ArrayView3<'_, f32>, angle: f64) -> Array3<f32> {
    let (height, width, channels) = a.dim();
    let angle = py_mod(angle, 360.0);
    if angle == 0.0 {
        return a.as_standard_layout().into_owned();
    }
    if angle == 180.0 {
        return rot90(a, 2);
    }
    if (angle == 90.0 || angle == 270.0) && width == height {
        // Transpose.ROTATE_90 turns counter-clockwise, ROTATE_270 clockwise.
        return rot90(a, if angle == 90.0 { 1 } else { 3 });
    }
    let m = rotate_matrix(angle, width, height);
    let source = a.as_standard_layout();
    let src = source.as_slice().unwrap_or(&[]);
    let mut out = vec![0.0f32; height * width * channels];
    if channels == 0 || width == 0 {
        return Array3::zeros((height, width, channels));
    }
    out.par_chunks_mut(width * channels).enumerate().for_each(|(y, row)| {
        let yin = y as f64 + 0.5;
        for (x, pixel) in row.chunks_mut(channels).enumerate() {
            let xin = x as f64 + 0.5;
            let xx = m[0] * xin + m[1] * yin + m[2];
            let yy = m[3] * xin + m[4] * yin + m[5];
            if !bicubic(src, width, height, channels, xx, yy, pixel) {
                pixel.fill(0.0);
            }
        }
    });
    Array3::from_shape_vec((height, width, channels), out).unwrap_or_else(|_| Array3::zeros((height, width, channels)))
}
