//! `cv2.bilateralFilter(plane, d, sigma_color, sigma_space)` on a float32 plane (OpenCV's `bilateralFilter_32f`
//! with its default reflect-101 border): the range weight comes from a 4096-bin table of exp(-x²/2σ²) over the
//! image's value range (its bin positions `i / scale` computed in float), linearly interpolated; the centre pixel
//! adds weight 1; the window is a disc. The arithmetic follows the AVX2 build: an 8-lane body with fused
//! multiply-adds, a scalar tail without.
//!
//! The oracle's opencv-python build on Windows replaces this filter with Intel IPP's (`ippiFilterBilateral`), whose
//! float32 internals are not documented; it differs from OpenCV's own by a few float32 ulps (measured up to
//! 6.1e-5 on L* 0..100). The Rust port reproduces OpenCV's own filter exactly (the oracle's result without IPP).

use grainy_core::Plane;
use rayon::prelude::*;

use super::reflect101;

/// Lanes of the vector body (AVX2 float32).
const LANES: usize = 8;
const BINS: usize = 1 << 12;

/// Bilateral filter of a float32 plane.
pub(crate) fn bilateral_filter(src: &Plane, d: i32, sigma_color: f64, sigma_space: f64) -> Plane {
    let (h, w) = src.dim();
    if h == 0 || w == 0 {
        return src.clone();
    }
    let sigma_color = if sigma_color <= 0.0 { 1.0 } else { sigma_color };
    let sigma_space = if sigma_space <= 0.0 { 1.0 } else { sigma_space };
    let color_coeff = -0.5 / (sigma_color * sigma_color);
    let space_coeff = -0.5 / (sigma_space * sigma_space);
    let radius = if d <= 0 { (sigma_space * 1.5).round_ties_even() as i32 } else { d / 2 }.max(1);

    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    for &v in src.iter() {
        lo = lo.min(f64::from(v));
        hi = hi.max(f64::from(v));
    }
    if (lo - hi).abs() < f64::from(f32::EPSILON) {
        return src.clone();
    }
    let len = (hi - lo) as f32;
    let scale_index = BINS as f32 / len;
    let mut lut = vec![0f32; BINS + 2];
    let mut last = 1f32;
    for (i, slot) in lut.iter_mut().enumerate() {
        if last > 0.0 {
            // `i / scale_index` is a float division in OpenCV (int / float), widened to double afterwards.
            let val = f64::from(i as f32 / scale_index);
            *slot = (val * val * color_coeff).exp() as f32;
            last = *slot;
        }
    }
    let mut weights = Vec::new();
    let mut offsets = Vec::new();
    for i in -radius..=radius {
        for j in -radius..=radius {
            let r = f64::from(i * i + j * j).sqrt();
            if r > f64::from(radius) || (i == 0 && j == 0) {
                continue;
            }
            weights.push((r * r * space_coeff).exp() as f32);
            offsets.push((i as isize, j as isize));
        }
    }
    let rad = radius as isize;
    // The bordered copy (copyMakeBorder with reflect-101).
    let bw = w + 2 * radius as usize;
    let bh = h + 2 * radius as usize;
    let mut temp = vec![0f32; bw * bh];
    for y in 0..bh {
        let sy = reflect101(y as isize - rad, h);
        for x in 0..bw {
            temp[y * bw + x] = src[[sy, reflect101(x as isize - rad, w)]];
        }
    }
    let body = w / LANES * LANES;
    let mut out = Plane::zeros((h, w));
    if let Some(data) = out.as_slice_mut() {
        data.par_chunks_mut(w).enumerate().for_each(|(y, dst)| {
            let centre = (y + radius as usize) * bw + radius as usize;
            let at = |k: usize, x: usize| {
                let (i, j) = offsets[k];
                temp[(centre as isize + i * bw as isize + j) as usize + x]
            };
            let mut sum = vec![0f32; w];
            let mut wsum = vec![0f32; w];
            // Vector body: v_trunc index, fused interpolation and accumulation (4 neighbours per step, then the
            // leftover neighbours one by one: the same operations in the same order). NaN neighbours are masked
            // out; a NaN centre gives the first table entry.
            for (k, &weight) in weights.iter().enumerate() {
                for x in 0..body {
                    let rval = temp[centre + x];
                    let val = at(k, x);
                    if val.is_nan() {
                        continue;
                    }
                    let alpha = if rval.is_nan() { 0.0 } else { (val - rval).abs() * scale_index };
                    let idx = (alpha as i32).clamp(0, BINS as i32);
                    let alpha = alpha - idx as f32;
                    let idx = idx as usize;
                    let wk = weight * lut[idx + 1].mul_add(alpha, lut[idx] * (1.0 - alpha));
                    wsum[x] += wk;
                    sum[x] = val.mul_add(wk, sum[x]);
                }
            }
            // Scalar tail: cvFloor index, `lut[i] + alpha*(lut[i+1] - lut[i])`, unfused.
            for x in body..w {
                let rval = temp[centre + x];
                for (k, &weight) in weights.iter().enumerate() {
                    let val = at(k, x);
                    if val.is_nan() {
                        continue;
                    }
                    let alpha = (val - rval).abs() * scale_index;
                    let idx = (alpha.floor() as i32).clamp(0, BINS as i32);
                    let alpha = alpha - idx as f32;
                    let idx = idx as usize;
                    let wk = weight * if rval.is_nan() { 1.0 } else { lut[idx] + alpha * (lut[idx + 1] - lut[idx]) };
                    wsum[x] += wk;
                    sum[x] += val * wk;
                }
            }
            for (x, slot) in dst.iter_mut().enumerate() {
                let val = temp[centre + x];
                *slot = if val.is_nan() { sum[x] / wsum[x] } else { (sum[x] + val) / (wsum[x] + 1.0) };
            }
        });
    }
    out
}
