//! Saturation, vibrance, the colour mixers (`luma/engine.py _develop_color_mix`, `luma/processing.py mix_hsl`)
//! and the colour tools (`luma/processing.py color_tools`).

use grainy_core::{parallel, Image};
use ndarray::Axis;

use crate::convert::{
    clip01, dot3, f, gemm3, hls_to_rgb_pixel, hsl_to_rgb_pixel, hsv_to_rgb_pixel, np_max, np_min, py_mod, py_mod64,
    rgb_to_hsl_pixel, rgb_to_hsv_pixel, simd_column, LUMA,
};
use crate::params::{any_set, ColorParams};
use crate::point_color::{self, PointHook};
use crate::Result;

/// Hue centres of the 8 mixer bands (turns): red, orange, yellow, green, aqua, blue, purple, magenta.
const MIXER_CENTRES: [f64; 8] = [0.0, 1.0 / 12.0, 1.0 / 6.0, 1.0 / 3.0, 0.5, 2.0 / 3.0, 0.75, 5.0 / 6.0];

/// Maps every pixel of an H×W×3 image through `f(column, width, pixel)`, in parallel row bands.
pub(crate) fn per_pixel<F>(image: &Image, f: F) -> Image
where
    F: Fn(usize, usize, [f32; 3]) -> [f32; 3] + Sync,
{
    let width = image.dim().1;
    parallel::map_rows(image, |band, mut out| {
        for (row, mut row_out) in band.axis_iter(Axis(0)).zip(out.axis_iter_mut(Axis(0))) {
            for (x, (p, mut o)) in row.axis_iter(Axis(0)).zip(row_out.axis_iter_mut(Axis(0))).enumerate() {
                let result = f(x, width, [p[0], p[1], p[2]]);
                o[0] = result[0];
                o[1] = result[1];
                o[2] = result[2];
            }
        }
    })
}

/// Port of `luma/engine.py _develop_color_mix`: saturation, vibrance and the colour mixer (HSV or HSL mode).
pub fn develop_color_mix(image: &Image, p: &ColorParams) -> Image {
    let mut a = if p.saturation != 0.0 || p.vibrance != 0.0 {
        saturation_vibrance(image, p.saturation, p.vibrance)
    } else {
        image.clone()
    };
    if any_set(&p.hsl) {
        a = if p.mixer_mode == "hsl" { mix_hsl(&a, &p.hsl) } else { mix_hsv(&a, &p.hsl) };
    }
    a
}

/// The saturation/vibrance part of `_develop_color_mix` (float32 as the oracle: the slider factors are Python
/// floats cast to float32).
pub fn saturation_vibrance(image: &Image, saturation: f64, vibrance: f64) -> Image {
    let factor = 1.0 + saturation / 100.0;
    let base = factor as f32;
    let vib = (vibrance / 100.0) as f32;
    per_pixel(image, |_, _, px| {
        let gray = dot3(&px, &LUMA);
        let factor = if vibrance != 0.0 {
            let sat = np_max(np_max(px[0], px[1]), px[2]) - np_min(np_min(px[0], px[1]), px[2]);
            base + vib * (1.0 - sat)
        } else {
            base
        };
        [gray + (px[0] - gray) * factor, gray + (px[1] - gray) * factor, gray + (px[2] - gray) * factor]
    })
}

/// Band weight of the mixers: `max(0, 1 - |(h - centre + .5) % 1 - .5| / width) ** power`.
#[inline]
fn band_weight(h: f32, centre: f32, width: f32) -> f32 {
    np_max(0.0, 1.0 - (py_mod(h - centre + 0.5, 1.0) - 0.5).abs() / width)
}

/// The HSV mixer of `luma/engine.py _develop_color_mix` (`mixer_mode` other than `"hsl"`).
pub fn mix_hsv(image: &Image, groups: &[Vec<f64>]) -> Image {
    per_pixel(image, |_, _, px| {
        let (h, sat, val) = rgb_to_hsv_pixel(&[clip01(px[0]), clip01(px[1]), clip01(px[2])]);
        let (mut dh, mut ds, mut dv) = (0f32, 0f32, 0f32);
        for (centre, group) in MIXER_CENTRES.iter().zip(groups) {
            let (hh, ss, ll) = (group[0], group[1], group[2]);
            if hh == 0.0 && ss == 0.0 && ll == 0.0 {
                continue;
            }
            let distance = (py_mod(h - f(*centre) + 0.5, 1.0) - 0.5).abs();
            let w = np_max(0.0, 1.0 - distance / f(1.0 / 8.0));
            let weight = w * w;
            if hh != 0.0 {
                dh += weight * hh as f32 / 600.0;
            }
            if ss != 0.0 {
                ds += weight * ss as f32 / 100.0;
            }
            if ll != 0.0 {
                dv += weight * ll as f32 / 200.0;
            }
        }
        hsv_to_rgb_pixel(py_mod(h + dh, 1.0), clip01(sat * (1.0 + ds)), clip01(val + dv))
    })
}

/// Port of `luma/processing.py mix_hsl`: the 8-band mixer in OpenCV's HLS.
pub fn mix_hsl(image: &Image, groups: &[Vec<f64>]) -> Image {
    per_pixel(image, |x, width, px| {
        let simd = simd_column(x, width);
        let (h, s, l) = rgb_to_hsl_pixel(&px, simd);
        let (mut dh, mut ds, mut dl) = (0f32, 0f32, 0f32);
        for (centre, group) in MIXER_CENTRES.iter().zip(groups) {
            let (hh, ss, ll) = (group[0], group[1], group[2]);
            if hh == 0.0 && ss == 0.0 && ll == 0.0 {
                continue;
            }
            let w = band_weight(h, f(*centre), 0.125);
            let weight = w * w;
            if hh != 0.0 {
                dh += weight * hh as f32 / 600.0;
            }
            if ss != 0.0 {
                ds += weight * ss as f32 / 100.0;
            }
            if ll != 0.0 {
                dl += weight * ll as f32 / 200.0;
            }
        }
        hsl_to_rgb_pixel(h + dh, s * (1.0 + ds), l + dl, simd)
    })
}

/// `np.allclose(x, y)` of float32 arrays (rtol 1e-5, atol 1e-8 in float32).
fn allclose(x: &[f32], y: &[f32]) -> bool {
    x.iter().zip(y).all(|(&a, &b)| {
        let close = (a - b).abs() <= f(1e-8) + f(1e-5) * b.abs() && b.is_finite();
        close || a == b
    })
}

/// A channel curve as `np.interp` evaluates it (float64), or `None` for the identity the oracle skips.
#[derive(Clone, Debug)]
pub struct ChannelCurve {
    xp: Vec<f64>,
    fp: Vec<f64>,
}

impl ChannelCurve {
    /// The curve of float32 points, `None` when `np.allclose(x, y)` (the oracle leaves the channel untouched).
    pub fn new(points: &[[f32; 2]]) -> Option<Self> {
        let x: Vec<f32> = points.iter().map(|p| p[0]).collect();
        let y: Vec<f32> = points.iter().map(|p| p[1]).collect();
        if allclose(&x, &y) {
            return None;
        }
        Some(Self { xp: x.iter().map(|&v| f64::from(v)).collect(), fp: y.iter().map(|&v| f64::from(v)).collect() })
    }

    /// `np.interp(value, xp, fp)` (points taken as sorted by x, as the curve editor keeps them).
    pub fn eval(&self, value: f32) -> f32 {
        interp(f64::from(value), &self.xp, &self.fp) as f32
    }
}

pub use grainy_core::numpy::interp;

/// The channel-curve step of `color_tools` (`a[..., c] = np.interp(a[..., c], x, y)` per non-identity curve).
pub fn channel_curves(image: &Image, curves: &[Vec<[f32; 2]>]) -> Image {
    let curves: Vec<Option<ChannelCurve>> = curves.iter().take(3).map(|p| ChannelCurve::new(p)).collect();
    if curves.iter().all(Option::is_none) {
        return image.clone();
    }
    per_pixel(image, |_, _, mut px| {
        for (c, curve) in curves.iter().enumerate() {
            if let Some(curve) = curve {
                px[c] = curve.eval(px[c]);
            }
        }
        px
    })
}

/// The parametric-curve step of `color_tools`: four luminance regions (centres .125/.375/.625/.875, half-width .375)
/// add an offset to all channels.
pub fn parametric_curve(image: &Image, parametric: &[f64]) -> Image {
    let centres = [0.125f64, 0.375, 0.625, 0.875];
    per_pixel(image, |_, _, px| {
        let lum = clip01(dot3(&px, &LUMA));
        let mut delta = 0f32;
        for (centre, &value) in centres.iter().zip(parametric) {
            let t = np_max(0.0, 1.0 - (lum - f(*centre)).abs() / f(0.375));
            delta += t * t * value as f32 / 400.0;
        }
        [px[0] + delta, px[1] + delta, px[2] + delta]
    })
}

/// The calibration step of `color_tools`: hue and saturation shifts around the red, green and blue primaries in
/// OpenCV's HLS (the result is clipped to 0..1 by the round trip).
pub fn calibration(image: &Image, calibration: &[Vec<f64>]) -> Image {
    let centres = [0.0f64, 1.0 / 3.0, 2.0 / 3.0];
    per_pixel(image, |x, width, px| {
        let simd = simd_column(x, width);
        let (h, sat, light) = rgb_to_hsl_pixel(&px, simd);
        let (mut dh, mut ds) = (0f32, 0f32);
        for (centre, group) in centres.iter().zip(calibration) {
            let (hue, amount) = (group[0], group[1]);
            if hue == 0.0 && amount == 0.0 {
                continue;
            }
            let w = band_weight(h, f(*centre), 0.25);
            if hue != 0.0 {
                dh += w * hue as f32 / 600.0;
            }
            if amount != 0.0 {
                ds += w * amount as f32 / 100.0;
            }
        }
        hsl_to_rgb_pixel(h + dh, sat * (1.0 + ds), light, simd)
    })
}

/// The RGB offset of one grading wheel: `(tint - .5) * sat / 200 + light / 400`, with the tint of the hue at full
/// saturation and lightness .5 (`hsl_to_rgb` of a 1×1 float64 array, so OpenCV's scalar code).
fn grading_offset(hue: f64, sat: f64, light: f64) -> [f32; 3] {
    let h = (py_mod64(hue / 360.0, 1.0) * 360.0) as f32;
    let tint = hls_to_rgb_pixel(h, 0.5, 1.0, false);
    let add = (light / 400.0) as f32;
    let sat = sat as f32;
    [(tint[0] - 0.5) * sat / 200.0 + add, (tint[1] - 0.5) * sat / 200.0 + add, (tint[2] - 0.5) * sat / 200.0 + add]
}

/// The colour grading step of `color_tools`: shadows, midtones and highlights wheels weighted by luminance shifted
/// by the balance.
pub fn grading(image: &Image, grading: &[Vec<f64>], balance: f64) -> Image {
    let offsets: Vec<[f32; 3]> = grading.iter().take(3).map(|g| grading_offset(g[0], g[1], g[2])).collect();
    let shift = (balance / 300.0) as f32;
    per_pixel(image, |_, _, mut px| {
        let l = clip01(dot3(&px, &LUMA) + shift);
        let weights = [(1.0 - l) * (1.0 - l), 4.0 * l * (1.0 - l), l * l];
        for (w, offset) in weights.iter().zip(&offsets) {
            for c in 0..3 {
                px[c] += w * offset[c];
            }
        }
        px
    })
}

/// The camera-matrix step of `color_tools`: `a @ matrix.T` (float32, as BLAS computes it).
pub fn camera_matrix(image: &Image, matrix: &[[f32; 3]; 3]) -> Image {
    per_pixel(image, |_, _, px| [gemm3(&px, &matrix[0]), gemm3(&px, &matrix[1]), gemm3(&px, &matrix[2])])
}

/// Port of `luma/processing.py color_tools`: channel curves, parametric curve, calibration, point colours, colour
/// grading and the camera matrix, then the clip to 0..1. `on_point_input` sees the input of each point colour
/// (and the result after the last one), as the oracle's picker callback.
pub fn color_tools(image: &Image, p: &ColorParams, on_point_input: Option<PointHook<'_>>) -> Result<Image> {
    let mut a = channel_curves(image, &p.rgb_curves);
    if p.parametric.iter().any(|&v| v != 0.0) {
        a = parametric_curve(&a, &p.parametric);
    }
    if any_set(&p.calibration) {
        a = calibration(&a, &p.calibration);
    }
    a = point_color::apply(a, &p.point_colors, on_point_input)?;
    if any_set(&p.grading) {
        a = grading(&a, &p.grading, p.grading_balance);
    }
    if let Some(matrix) = &p.camera_matrix {
        a = camera_matrix(&a, matrix);
    }
    a.mapv_inplace(clip01);
    Ok(a)
}
