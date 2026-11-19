use crate::{invalid, plane, Result};
use grainy_core::Plane;
use grainy_imgproc as cv;
use ndarray::{s, Array2};

/// Port of luma/dust.py _median (float32 addition before conversion to float64).
pub fn median(values: &[f32]) -> Result<f64> {
    let mut v = ordered(values)?;
    let m = v.len() / 2;
    v.select_nth_unstable_by(m, f32::total_cmp);
    Ok(if v.len() % 2 == 1 {
        v[m] as f64
    } else {
        let lo = v[..m].iter().copied().fold(f32::NEG_INFINITY, f32::max);
        ((lo + v[m]) * 0.5) as f64
    })
}
fn ordered(values: &[f32]) -> Result<Vec<f32>> {
    if values.is_empty() || values.iter().any(|x| !x.is_finite()) {
        return Err(invalid("expected finite nonempty vector"));
    }
    Ok(values.to_vec())
}
/// Port of luma/dust.py _quantile_plan arrays; scalar percentiles are float32 on the oracle NumPy 2.5.3.
#[derive(Debug, serde::Serialize)]
pub struct QuantilePlan {
    /// Port of _quantile_plan kth.
    pub kth: Vec<usize>,
    /// Port of _quantile_plan lower.
    pub lower: Vec<usize>,
    /// Port of _quantile_plan upper.
    pub upper: Vec<usize>,
    /// Port of _quantile_plan backward.
    pub backward: Vec<bool>,
    /// Port of _quantile_plan inverse.
    pub inverse: Vec<f32>,
    /// Port of _quantile_plan weight.
    pub weight: Vec<f32>,
}
/// Port of luma/dust.py _quantile_plan. No mutable global cache is needed for correctness.
pub fn quantile_plan(size: usize, fractions: &[f64]) -> Result<QuantilePlan> {
    if size == 0 || fractions.iter().any(|f| !f.is_finite() || !(0. ..=1.).contains(f)) {
        return Err(invalid("quantile size or fractions"));
    }
    let positions: Vec<_> = fractions.iter().map(|f| (size - 1) as f64 * f).collect();
    let lower: Vec<_> = positions.iter().map(|p| *p as usize).collect();
    let upper: Vec<_> = lower.iter().map(|p| (p + 1).min(size - 1)).collect();
    let weights: Vec<_> = positions.iter().zip(&lower).map(|(p, l)| p - *l as f64).collect();
    Ok(QuantilePlan {
        kth: lower.iter().chain(&upper).copied().collect(),
        lower,
        upper,
        backward: weights.iter().map(|w| *w >= 0.5).collect(),
        inverse: weights.iter().map(|w| (1. - w) as f32).collect(),
        weight: weights.iter().map(|w| *w as f32).collect(),
    })
}
/// Port of luma/dust.py _quantiles.
pub fn quantiles(values: &[f32], fractions: &[f64]) -> Result<Vec<f32>> {
    let plan = quantile_plan(values.len(), fractions)?;
    let mut v = ordered(values)?;
    v.sort_unstable_by(f32::total_cmp);
    Ok((0..fractions.len())
        .map(|i| {
            let a = v[plan.lower[i]];
            let b = v[plan.upper[i]];
            let d = b - a;
            if plan.backward[i] {
                b - d * plan.inverse[i]
            } else {
                a + d * plan.weight[i]
            }
        })
        .collect())
}
/// Port of luma/dust.py _component_indices; ownership is [left,top,right,bottom].
pub fn component_indices(
    stats: &Array2<i32>,
    minimum: f64,
    maximum: f64,
    origin: (usize, usize),
    ownership: [usize; 4],
) -> Result<Vec<usize>> {
    if stats.ncols() != 5 || !minimum.is_finite() || !maximum.is_finite() {
        return Err(invalid("component statistics"));
    }
    let [left, top, right, bottom] = ownership;
    let (x0, y0) = origin;
    Ok(stats
        .rows()
        .into_iter()
        .enumerate()
        .skip(1)
        .filter_map(|(i, r)| {
            let (x, y, w, h, area) = (r[0] as f64, r[1] as f64, r[2] as f64, r[3] as f64, r[4] as f64);
            let small = w.min(h);
            let large = w.max(h);
            let gx = x + (w - 1.) / 2. + x0 as f64;
            let gy = y + (h - 1.) / 2. + y0 as f64;
            (small > 0.
                && large <= maximum
                && (4. * area / std::f64::consts::PI).sqrt() >= minimum
                && large / small.max(1.) <= 3.5
                && area / (w * h).max(1.) >= 0.3
                && gx >= left as f64
                && gx < right as f64
                && gy >= top as f64
                && gy < bottom as f64)
                .then_some(i)
        })
        .collect())
}

// NumPy's contiguous pairwise summation: preserve float32 std/mean decisions.
pub(crate) fn sum(v: &[f32]) -> f32 {
    if v.len() < 8 {
        return v.iter().fold(-0.0, |a, b| a + b);
    }
    if v.len() <= 128 {
        let mut r = [0.; 8];
        r.copy_from_slice(&v[..8]);
        let mut i = 8;
        while i + 8 <= v.len() {
            for j in 0..8 {
                r[j] += v[i + j];
            }
            i += 8;
        }
        let mut s = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for x in &v[i..] {
            s += x;
        }
        s
    } else {
        let mut n = v.len() / 2;
        n -= n % 8;
        sum(&v[..n]) + sum(&v[n..])
    }
}
/// Port of luma/dust.py _repeated_detail.
pub fn repeated_detail(image: &Plane, rect: [usize; 4], reduce_patterns: bool) -> Result<bool> {
    plane(image)?;
    let [x, y, w, h] = rect;
    let (height, width) = image.dim();
    if w == 0 || h == 0 || x.checked_add(w).is_none_or(|r| r > width) || y.checked_add(h).is_none_or(|b| b > height) {
        return Err(invalid("template rectangle"));
    }
    if x.min(y).min(width - x - w).min(height - y - h) < 2 {
        return Ok(false);
    }
    let template = image.slice(s![y - 2..y + h + 2, x - 2..x + w + 2]).to_owned();
    let values: Vec<_> = template.iter().copied().collect();
    let mean = sum(&values) / values.len() as f32;
    let variance: Vec<_> = values.iter().map(|v| (v - mean) * (v - mean)).collect();
    if (sum(&variance) / values.len() as f32).sqrt() < 0.004 {
        return Ok(false);
    }
    let pad = 24.max(w.max(h) * 4);
    let left = x.saturating_sub(pad);
    let top = y.saturating_sub(pad);
    let right = width.min(x + w + pad);
    let bottom = height.min(y + h + pad);
    let source = image.slice(s![top..bottom, left..right]).to_owned();
    let mut corr = cv::match_template(&source, &template, cv::TemplateMatch::CCoeffNormed)?;
    let own_x = x - 2 - left;
    let own_y = y - 2 - top;
    let (ch, cw) = corr.dim();
    if !reduce_patterns {
        let mut peaks = corr.mapv(|v| u8::from(v > 0.94));
        for yy in own_y.saturating_sub(h * 2)..(own_y + h * 2 + 1).min(ch) {
            for xx in own_x.saturating_sub(w * 2)..(own_x + w * 2 + 1).min(cw) {
                peaks[(yy, xx)] = 0;
            }
        }
        return Ok(cv::connected_components(&peaks, 8)?.0 > 3);
    }
    for yy in own_y.saturating_sub(h * 2)..(own_y + h * 2 + 1).min(ch) {
        for xx in own_x.saturating_sub(w * 2)..(own_x + w * 2 + 1).min(cw) {
            corr[(yy, xx)] = -1.;
        }
    }
    let dilated = cv::dilate(&corr, &Array2::ones((3, 3)), None, 1, None)?;
    let mut peaks: Vec<_> = corr
        .indexed_iter()
        .filter(|&((yy, xx), &v)| v > 0.75 && v >= dilated[(yy, xx)])
        .map(|(p, &v)| (p, v))
        .collect();
    if peaks.len() < 3 {
        return Ok(false);
    }
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
    let range =
        values.iter().copied().fold(f32::NEG_INFINITY, f32::max) - values.iter().copied().fold(f32::INFINITY, f32::min);
    let tolerance = 0.006f64.max(0.02f64.min(range as f64 * 0.12));
    let mut matches: Vec<(usize, usize)> = Vec::new();
    for &((j, i), _) in peaks.iter().take(24) {
        if matches
            .iter()
            .any(|&(a, b)| (i as f64 - a as f64).powi(2) + (j as f64 - b as f64).powi(2) < w.max(h).pow(2) as f64)
        {
            continue;
        }
        let mut border = Vec::new();
        let mut core = Vec::new();
        for yy in 0..h + 4 {
            for xx in 0..w + 4 {
                let d = template[(yy, xx)] - image[(top + j + yy, left + i + xx)];
                if yy < 2 || yy >= h + 2 || xx < 2 || xx >= w + 2 {
                    border.push(d);
                } else {
                    core.push(d);
                }
            }
        }
        let offset = median(&border)? as f32;
        let error = median(&core.iter().map(|d| (d - offset).abs()).collect::<Vec<_>>())?;
        if error <= tolerance {
            matches.push((i, j));
        }
        if matches.len() >= 3 {
            return Ok(true);
        }
    }
    Ok(false)
}
