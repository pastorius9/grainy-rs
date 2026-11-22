//! Process Version 4 (`tone_version >= 4`, Grainy RS Native):
//! - Unclipped scene-linear highlight recovery and shadow lift in 12-stop log-luminance (`[-8 EV .. +4 EV]`)
//!   guided by an edge-preserving filter (`ToneBase`), preserving RGB chromaticity and local micro-contrast.
//! - Hue-preserving C³ filmic shoulder compression (`peak / (1 + p^4)^(1/4)`) with subtle optical highlight
//!   desaturation so blown skies, skin highlights, and speculars never hue-twist.
//!
//! All arithmetic is portable f32 shared identically with `grainy-gpu/src/tone.wgsl`.

use grainy_core::{Image, Plane};
use ndarray::ArrayView3;

use crate::cv::{box_filter, LinearColumns};
use crate::num::{clip, round_half_even};
use crate::response::{luma, ToneBase, MAP_SIZE};

/// Lowest log2 luminance mapped by the Process v4 guided base (-8 EV).
pub const LOG_MIN: f32 = -8.0;
/// Log2 dynamic range spanned by the Process v4 guided base ([-8 EV .. +4 EV] = 12 stops).
pub const LOG_SPAN: f32 = 12.0;
/// Linear threshold below which the Process v4 filmic shoulder is a 100% linear identity.
pub const SHOULDER_KNEE_V4: f32 = 0.72;
/// Guided-filter radius fraction of the reduced map's long edge.
pub const RADIUS_V4: f64 = 0.025;
/// Guided-filter edge regularization in normalized log-luminance.
pub const EPS_V4: f32 = 0.012;

#[inline]
pub fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[inline]
pub fn log_u(y: f32) -> f32 {
    (((y.max(0.0) + 1e-4_f32).log2() - LOG_MIN) / LOG_SPAN).clamp(0.0, 1.0)
}

/// Edge-preserving guided filter on normalized 12-stop log-luminance of the reduced linear frame.
pub fn analyse(linear_small: &Image, shape: (usize, usize)) -> ToneBase {
    let (h, w, _) = linear_small.dim();
    let mut u_plane = Plane::zeros((h, w));
    for ((y, x), slot) in u_plane.indexed_iter_mut() {
        let p = [linear_small[[y, x, 0]].max(0.0), linear_small[[y, x, 1]].max(0.0), linear_small[[y, x, 2]].max(0.0)];
        *slot = log_u(luma(&p));
    }
    let r = (round_half_even(RADIUS_V4 * h.max(w) as f64) as usize).max(1);
    let mean = box_filter(&u_plane, r);
    let squares = u_plane.mapv(|v| v * v);
    let mean_sq = box_filter(&squares, r);
    let mut a = Plane::zeros((h, w));
    let mut b = Plane::zeros((h, w));
    for ((index, &m), &s) in mean.indexed_iter().zip(mean_sq.iter()) {
        let variance = (s - m * m).max(0.0);
        let av = variance / (variance + EPS_V4);
        a[index] = av;
        b[index] = m - av * m;
    }
    ToneBase { a: box_filter(&a, r), b: box_filter(&b, r), shape }
}

/// Apply Process v4 scene-linear Highlight Recovery and Shadow Lift to one linear RGB pixel.
#[inline]
pub fn apply_local_pixel(rgb: [f32; 3], k_a: f32, k_b: f32, highlights: f32, shadows: f32) -> [f32; 3] {
    let r = rgb[0].max(0.0);
    let g = rgb[1].max(0.0);
    let b = rgb[2].max(0.0);
    let y = luma(&[r, g, b]);
    let u = log_u(y);
    let base_u = (k_a * u + k_b).clamp(0.0, 1.0);
    let w_hl = smoothstep(0.42, 0.72, base_u);
    let w_sh = (1.0 - smoothstep(0.18, 0.50, base_u)) * smoothstep(0.0, 0.08, base_u);
    let hl = (highlights / 100.0).clamp(-1.0, 1.0);
    let sh = (shadows / 100.0).clamp(-1.0, 1.0);
    let delta_ev = sh * 2.2 * w_sh + hl * 2.4 * w_hl;
    let gain = delta_ev.exp2();
    [r * gain, g * gain, b * gain]
}

/// Hue-preserving C³ filmic shoulder compression with gentle optical highlight desaturation.
/// Values with `max(r, g, b) <= 0.72` are returned unchanged; values up to 16+ roll smoothly into `[0.72, 1.0)`.
#[inline]
pub fn filmic_shoulder_pixel(rgb: [f32; 3]) -> [f32; 3] {
    let r = rgb[0].max(0.0);
    let g = rgb[1].max(0.0);
    let b = rgb[2].max(0.0);
    let peak = r.max(g).max(b);
    if peak <= SHOULDER_KNEE_V4 {
        return [r, g, b];
    }
    let headroom = 1.0 - SHOULDER_KNEE_V4;
    let p = (peak - SHOULDER_KNEE_V4) / headroom;
    let p2 = p * p;
    let s_peak = SHOULDER_KNEE_V4 + headroom * p / (1.0 + p2 * p2).sqrt().sqrt();
    let scale = s_peak / peak;
    let d = ((peak - 0.85) / 2.2).clamp(0.0, 1.0);
    let desat = d * d * 0.65;
    let mix = |c: f32| {
        let scaled = c * scale;
        scaled + (s_peak - scaled) * desat
    };
    [mix(r), mix(g), mix(b)]
}

/// Apply Process v4 local Highlights/Shadows across a linear band `first..first+count`.
pub fn apply_local_band(a: &mut Image, base: &ToneBase, first: usize, highlights: f32, shadows: f32) {
    let (count, width, _) = a.dim();
    let shape = base.shape;
    let same = base.a.dim() == shape;
    let (rows, cols) = base.a.dim();
    let columns = (!same && cols != shape.1).then(|| LinearColumns::new(cols, shape.1));
    let scale_y = (rows as f64 / shape.0.max(1) as f64) as f32;
    let mut band_a = vec![0.0_f64; width];
    let mut band_b = vec![0.0_f64; width];
    for i in 0..count {
        let y = first + i;
        if !same {
            let fy = clip((y as f32 + 0.5) * scale_y - 0.5, 0.0, (rows - 1) as f32);
            let upper = fy.floor() as usize;
            let lower = (upper + 1).min(rows - 1);
            let ty = f64::from(fy) - upper as f64;
            let row_a: Vec<f64> = (0..cols)
                .map(|x| f64::from(base.a[[upper, x]]) * (1.0 - ty) + f64::from(base.a[[lower, x]]) * ty)
                .collect();
            let row_b: Vec<f64> = (0..cols)
                .map(|x| f64::from(base.b[[upper, x]]) * (1.0 - ty) + f64::from(base.b[[lower, x]]) * ty)
                .collect();
            match &columns {
                Some(table) => {
                    table.row(&row_a, &mut band_a);
                    table.row(&row_b, &mut band_b);
                }
                None => {
                    band_a.copy_from_slice(&row_a);
                    band_b.copy_from_slice(&row_b);
                }
            }
        }
        for x in 0..width {
            let p = [a[[i, x, 0]], a[[i, x, 1]], a[[i, x, 2]]];
            let (ka, kb) = if same { (base.a[[y, x]], base.b[[y, x]]) } else { (band_a[x] as f32, band_b[x] as f32) };
            let out = apply_local_pixel(p, ka, kb, highlights, shadows);
            a[[i, x, 0]] = out[0];
            a[[i, x, 1]] = out[1];
            a[[i, x, 2]] = out[2];
        }
    }
}

/// Reduce a full linear frame to `MAP_SIZE` on the long edge (matching `tone_base`).
pub fn reduce_for_base(a: ArrayView3<'_, f32>) -> crate::Result<Image> {
    let (height, width, _) = a.dim();
    let owned = a.as_standard_layout().into_owned();
    let scale = MAP_SIZE as f64 / height.max(width) as f64;
    if scale < 1.0 {
        let w = (round_half_even(width as f64 * scale) as usize).max(1);
        let h = (round_half_even(height as f64 * scale) as usize).max(1);
        crate::cv::resize_area(&owned, w, h).ok_or(crate::Error::Resize)
    } else {
        Ok(owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::develop_tone;
    use grainy_core::defaults;

    #[test]
    fn pv4_recovers_unclipped_linear_highlights_and_preserves_hue() {
        // Synthetic bright sky/cloud gradient from +0.5 EV (1.4) up to +2.5 EV (5.6) in linear light.
        let image = Image::from_shape_fn((32, 64, 3), |(_, x, c)| {
            let t = x as f32 / 63.0;
            let base = 1.4 + 4.2 * t;
            match c {
                0 => base * 1.0,
                1 => base * 0.75,
                _ => base * 0.40,
            }
        });

        let mut s_v3 = defaults();
        s_v3.set("tone_version", 3);
        s_v3.set("highlights", -100.0);
        let out_v3 = develop_tone(&image, &s_v3, None).unwrap();

        let mut s_v4 = defaults();
        s_v4.set("tone_version", 4);
        s_v4.set("highlights", -100.0);
        let out_v4 = develop_tone(&image, &s_v4, None).unwrap();

        // In v3, values > 1.0 are hard-clipped before Highlights runs, so x=0 (1.4) and x=63 (5.6)
        // have zero surviving red separation (both clipped before Highlights).
        let v3_r_span = (out_v3[[16, 63, 0]] - out_v3[[16, 0, 0]]).abs();
        let v4_r_span = (out_v4[[16, 63, 0]] - out_v4[[16, 0, 0]]).abs();
        assert!(
            v4_r_span > v3_r_span + 0.08,
            "PV4 must preserve unclipped linear highlight gradation (v4 span {v4_r_span}, v3 span {v3_r_span})"
        );

        // Hue invariance of filmic shoulder: warm golden highlight [1.2, 0.84, 0.36] keeps exact
        // ratio order R > G > B and does not shift G/R upward like per-channel rolloff.
        let warm = [1.2_f32, 0.84, 0.36];
        let sh = filmic_shoulder_pixel(warm);
        assert!(sh[0] > sh[1] && sh[1] > sh[2]);
        assert!(sh[0] < 1.0, "shoulder stays below 1.0, got {sh:?}");
        // Below knee (0.72), filmic shoulder is a bit-exact identity.
        let mid = [0.50_f32, 0.35, 0.20];
        assert_eq!(filmic_shoulder_pixel(mid), mid);
    }
}
