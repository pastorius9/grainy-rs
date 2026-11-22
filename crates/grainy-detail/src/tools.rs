//! The detail tools of luma/processing.py: `blur`, `detail_tools` (noise reduction, dehaze, texture, defringe)
//! and `sharpen`. float32 throughout, Python float scalars applied as float32 (numpy's weak scalars).

use grainy_core::{Image, Plane, Settings};
use ndarray::{Axis, Zip};

use crate::cv::{self, Mat};
use crate::noise::{self, WAVELET_BINS};
use crate::np::{self, clip, dot3, LUMA};

/// Port of luma/processing.py `blur`: `cv2.GaussianBlur(a, (0, 0), max(0.05, sigma))` with reflect-101 borders.
pub fn blur(a: &Image, sigma: f64) -> Image {
    cv::gaussian_blur(&Mat::from_image(a), sigma.max(0.05)).into_image()
}

/// The detail tool radius in original pixels: `2·round(3·pixel_scale) + 1` (Python's round half to even).
fn diameter(pixel_scale: f64) -> usize {
    2 * (3.0 * pixel_scale).round_ties_even().max(0.0) as usize + 1
}

/// The `detail_version` the oracle reads with `s.get('detail_version', 1)`.
pub(crate) fn detail_version(s: &Settings) -> i64 {
    match s.0.get("detail_version") {
        Some(_) => s.i64("detail_version"),
        None => 1,
    }
}

/// The `wavelet_profiles` a local adjustment passes in its settings (`s.get('wavelet_profiles')`), if any.
fn settings_profiles(s: &Settings) -> Option<Vec<[f32; WAVELET_BINS]>> {
    let rows = s.0.get("wavelet_profiles")?.as_array()?;
    rows.iter()
        .map(|row| {
            let values = row.as_array()?;
            let mut out = [0f32; WAVELET_BINS];
            for (slot, v) in out.iter_mut().zip(values) {
                *slot = v.as_f64()? as f32;
            }
            Some(out)
        })
        .collect()
}

/// Port of luma/processing.py `detail_tools`: luminance and colour noise reduction (in Lab), dehaze, texture and
/// defringe, then a clip to [0, 1]. Radii are measured in original pixels (`pixel_scale` < 1 for reduced
/// previews).
pub fn detail_tools(a: &Image, s: &Settings, pixel_scale: f64) -> Image {
    let d = diameter(pixel_scale);
    let noise_luma = s.f64("noise_luma");
    let noise_color = s.f64("noise_color");
    let mut a = a.as_standard_layout().into_owned();
    if d > 1 && (noise_luma != 0.0 || noise_color != 0.0) {
        let mut lab = grainy_color::rgb_to_lab(&a);
        // Colour first: its guide is the original luminance.
        if noise_color != 0.0 {
            noise::chroma_guided(&mut lab, noise_color, pixel_scale);
        }
        if noise_luma != 0.0 {
            let lightness = lab.index_axis(Axis(2), 0).to_owned();
            let filtered = if detail_version(s) >= 3 {
                let profiles = settings_profiles(s);
                noise::luma_wavelet(&lightness, noise_luma, pixel_scale, profiles.as_deref())
            } else {
                noise::luma_noise_reduction(&lightness, noise_luma, pixel_scale)
            };
            lab.index_axis_mut(Axis(2), 0).assign(&filtered);
        }
        a = grainy_color::lab_to_rgb(&lab);
    }
    let v4 = detail_version(s) >= 4;
    let dehaze = s.f64("dehaze");
    if dehaze != 0.0 {
        if v4 {
            crate::laplacian::guided_dehaze(&mut a, dehaze, d);
        } else {
            let strength = (dehaze / 100.0 * 0.8) as f32;
            let darkest = a.map_axis(Axis(2), |p| np::minimum(np::minimum(p[0], p[1]), p[2]));
            let dark = cv::erode(&darkest, d);
            Zip::from(a.rows_mut()).and(&dark).par_for_each(|mut p, &dk| {
                let transmission = clip(1.0 - strength * dk, 0.2, 1.8);
                p.mapv_inplace(|v| (v - 1.0) / transmission + 1.0);
            });
        }
    }
    let texture = s.f64("texture");
    if texture != 0.0 {
        if v4 {
            crate::laplacian::edge_aware_texture(&mut a, texture, pixel_scale);
        } else {
            let blurred = blur(&a, 1.3 * pixel_scale);
            let t = texture as f32;
            Zip::from(&mut a).and(&blurred).par_for_each(|v, &b| *v += (*v - b) * t / 100.0);
        }
    }
    let defringe = s.f64("defringe");
    if defringe != 0.0 {
        let maximum: Plane = a.map_axis(Axis(2), |p| np::maximum(np::maximum(p[0], p[1]), p[2]));
        let edge = cv::laplacian(&maximum);
        let ps2 = (pixel_scale * pixel_scale) as f32;
        let amount = defringe as f32;
        Zip::from(a.rows_mut()).and(&edge).par_for_each(|mut p, &e| {
            let purple = (p[0] + p[2]) / 2.0 - p[1];
            let edge = e.abs() * ps2;
            let weight = clip(purple * 6.0, 0.0, 1.0) * clip(edge * 3.0, 0.0, 1.0) * amount / 100.0;
            let mean = (p[0] + p[1] + p[2]) / 3.0;
            let keep = 1.0 - weight;
            p.mapv_inplace(|v| v * keep + mean * weight);
        });
    }
    a.mapv_inplace(|v| clip(v, 0.0, 1.0));
    a
}

/// Port of luma/processing.py `sharpen`: unsharp mask of `radius·pixel_scale`, residual thresholded by `detail`
/// (below 100), limited to edges by `masking` (Sobel magnitude of the luminance), added `amount/65` times.
/// `luminance` (detail_version ≥ 2) adds the residual's luminance to all three channels.
pub fn sharpen(
    a: &Image,
    amount: f64,
    radius: f64,
    detail: f64,
    masking: f64,
    pixel_scale: f64,
    luminance: bool,
) -> Image {
    let blurred = blur(a, radius * pixel_scale);
    let (h, w, _) = a.dim();
    // The residual: three channels, or one (its luminance) broadcast to all three.
    let channels = if luminance { 1 } else { 3 };
    let mut residual = Image::zeros((h, w, channels));
    Zip::from(residual.rows_mut()).and(a.rows()).and(blurred.rows()).par_for_each(|mut r, p, b| {
        let d = [p[0] - b[0], p[1] - b[1], p[2] - b[2]];
        if luminance {
            r[0] = dot3(&d, &LUMA);
        } else {
            r.iter_mut().zip(d).for_each(|(slot, v)| *slot = v);
        }
    });
    if detail < 100.0 {
        let threshold = ((1.0 - detail / 100.0) * 0.012).max(1e-6) as f32;
        residual.mapv_inplace(|v| v * clip(v.abs() / threshold, 0.0, 1.0));
    }
    if masking != 0.0 {
        let gray: Plane = a.map_axis(Axis(2), |p| dot3(&[p[0], p[1], p[2]], &LUMA));
        let gx = cv::sobel_x(&gray);
        let gy = cv::sobel_y(&gray);
        let ps = pixel_scale as f32;
        let limit = (0.002 + masking / 200.0) as f32;
        Zip::from(residual.rows_mut()).and(&gx).and(&gy).par_for_each(|mut r, &x, &y| {
            let edge = x.hypot(y) * ps;
            let m = clip(edge / limit, 0.0, 1.0);
            r.mapv_inplace(|v| v * m);
        });
    }
    let amount = amount as f32;
    let mut out = a.as_standard_layout().into_owned();
    Zip::from(out.rows_mut()).and(residual.rows()).par_for_each(|mut p, r| {
        for (c, v) in p.iter_mut().enumerate() {
            let rc = if luminance { r[0] } else { r[c] };
            *v += rc * amount / 65.0;
        }
    });
    out
}
