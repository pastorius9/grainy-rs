//! Port of the parts of luma/hdr.py that the tone stage and `develop` reach. The HDR editing mode is switched
//! off in the shipped oracle (`engine.HDR_FEATURE = False`), so these only run for settings that bypass
//! `normalized()`.

use grainy_core::{Image, Settings};

use crate::interp::interp;
use crate::num::{clip, maximum, minimum, pixels_mut, Float};
use crate::srgb::{to_linear_value, to_srgb_value};

/// A point curve read as float32 pairs (`np.asarray(points, np.float32)`).
pub type Points = Vec<[f32; 2]>;

/// The extended curve of one value: `np.interp` below the last point, the last segment's slope above it.
pub(crate) fn curve_extended_value<T: Float>(v: T, points: &Points, xs: &[f64], ys: &[f64], slope: f64) -> T {
    let [last_x, last_y] = points[points.len() - 1];
    if v > T::of(f64::from(last_x)) {
        T::of(f64::from(last_y)) + (v - T::of(f64::from(last_x))) * T::of(slope)
    } else {
        // np.interp(...).astype(np.float32); a float64 input then meets the float32 result in np.where.
        T::of(f64::from(interp(v.to_f64(), xs, ys) as f32))
    }
}

/// The last segment's slope (`max(0., float((p[-1,1]-p[-2,1])/max(p[-1,0]-p[-2,0],1e-6)))`).
pub(crate) fn extension_slope(points: &Points) -> f64 {
    let n = points.len();
    if n < 2 {
        return 0.0; // the oracle raises an IndexError for a single point
    }
    let rise = points[n - 1][1] - points[n - 2][1];
    let run = points[n - 1][0] - points[n - 2][0];
    // Python's max(run, 1e-6) compares in float32 and returns the larger object.
    let tiny = 1e-6_f64 as f32;
    let run = if tiny > run { tiny } else { run };
    f64::from(rise / run).max(0.0)
}

/// Port of luma/hdr.py `curve_extended`: the point curve, keeping the endpoint's slope above SDR white.
pub fn curve_extended(pixels: &Image, points: &Points) -> Image {
    let xs: Vec<f64> = points.iter().map(|p| f64::from(p[0])).collect();
    let ys: Vec<f64> = points.iter().map(|p| f64::from(p[1])).collect();
    let slope = extension_slope(points);
    let mut out = pixels.as_standard_layout().into_owned();
    if points.is_empty() {
        return out;
    }
    pixels_mut(&mut out, |p| p.iter_mut().for_each(|v| *v = curve_extended_value(*v, points, &xs, &ys, slope)));
    out
}

/// Port of luma/hdr.py `limit`: HDR brightness and the ceiling on sRGB-encoded extended pixels.
pub fn limit(pixels: &Image, settings: &Settings) -> Image {
    let gain = 2f64.powf(settings.f64("hdr_brightness").clamp(-100.0, 100.0) / 100.0) as f32;
    let ceiling = 2f64.powf(settings.f64("hdr_limit").clamp(0.0, 4.0)) as f32;
    let mut out = pixels.as_standard_layout().into_owned();
    pixels_mut(&mut out, |p| {
        for v in p.iter_mut() {
            let linear = to_linear_value(maximum(*v, 0.0));
            let stops = maximum(linear, 1.0).log2();
            let linear = if linear > 1.0 { 2f32.powf(stops * gain) } else { linear };
            *v = to_srgb_value(minimum(linear, ceiling));
        }
    });
    out
}

/// Port of luma/hdr.py `sdr_rendition`: a continuous, monotonic shoulder in linear light, then sRGB, 0..1.
pub fn sdr_rendition(pixels: &Image, settings: &Settings) -> Image {
    let exposure = 2f64.powf(settings.f64("sdr_exposure").clamp(-5.0, 5.0)) as f32;
    let knee = 1.0 - 0.9 * settings.f64("sdr_compression").clamp(0.0, 100.0) / 100.0;
    let span = (1.0 - knee).max(1e-6);
    let (knee32, span32) = (knee as f32, span as f32);
    let mut out = pixels.as_standard_layout().into_owned();
    pixels_mut(&mut out, |p| {
        let a = [to_linear_value(p[0]) * exposure, to_linear_value(p[1]) * exposure, to_linear_value(p[2]) * exposure];
        let peak = maximum(maximum(maximum(a[0], a[1]), a[2]), 0.0);
        let delta = maximum(peak - knee32, 0.0);
        let shoulder = knee32 + span32 * delta / (delta + span32);
        let mapped = if peak > knee32 { shoulder } else { peak };
        let factor = if peak > 1e-8_f64 as f32 { mapped / peak } else { 1.0 };
        for c in 0..3 {
            p[c] = clip(to_srgb_value(a[c] * factor), 0.0, 1.0);
        }
    });
    out
}

/// Port of luma/hdr.py `color_operation`: a bounded colour operation on pixels divided by their highlight gain
/// (the largest channel, at least 1), multiplied back afterwards.
pub fn color_operation<F>(pixels: &Image, operation: F) -> Image
where
    F: FnOnce(&Image) -> Image,
{
    let mut gain = pixels.as_standard_layout().into_owned();
    pixels_mut(&mut gain, |p| {
        let g = maximum(1.0, maximum(maximum(p[0], p[1]), p[2]));
        p.iter_mut().for_each(|v| *v = g);
    });
    let mut bounded = pixels.as_standard_layout().into_owned();
    ndarray::Zip::from(&mut bounded).and(&gain).par_for_each(|v, g| *v /= g);
    let mut result = operation(&bounded);
    if result.dim() == gain.dim() {
        ndarray::Zip::from(&mut result).and(&gain).par_for_each(|v, g| *v *= g);
    }
    result
}
