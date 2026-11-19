//! Port of luma/upright.py: deterministic line/vanishing-point alignment.
//!
//! The oracle's `estimate(image, mode)` finds segments with OpenCV's LSD detector on the display-encoded
//! luminance; here [`estimate_from_lines`] takes those segments (N×4 `float32`, `x0 y0 x1 y1`, as
//! `detect()[0].reshape(-1, 4)` returns them; `None` when the detector found nothing) and runs the rest.
//! [`apply`] warps a frame (or mask) with a stored result. Errors carry the oracle's Korean messages.

use std::borrow::Cow;

use grainy_core::Settings;
use grainy_imgproc::{get_rotation_matrix_2d, warp_perspective, Border, Interpolation};
use ndarray::{Array3, ArrayView2};
use serde_json::{json, Value};

use crate::numeric::{
    argmax, argsort, convolve_ones5, det, dot3, histogram, inv, masked_sum, matmul, matvec, median, pairwise_f64, ptp,
    py_mod, searchsorted, EYE, M3,
};
use crate::rng;
use crate::{Error, Result};

/// A segment: its two end points `[[x0, y0], [x1, y1]]`.
pub type Line = [[f64; 2]; 2];

fn alignment<T>(message: &str) -> Result<T> {
    Err(Error::Alignment(message.to_owned()))
}

/// Port of `_points`: points through a projective matrix; an error when one maps to infinity.
pub fn points(matrix: &M3, lines: &[Line]) -> Result<Vec<Line>> {
    let mut projected = Vec::with_capacity(lines.len());
    for line in lines {
        let p: [[f64; 3]; 2] = std::array::from_fn(|k| {
            let v = [line[k][0], line[k][1], 1.0];
            [dot3(v, matrix[0]), dot3(v, matrix[1]), dot3(v, matrix[2])]
        });
        projected.push(p);
    }
    if projected.iter().flatten().any(|p| p[2].abs() < 1e-8) {
        return alignment("원근 보정 범위가 너무 큽니다. 가이드를 사용해 주세요.");
    }
    Ok(projected.iter().map(|l| std::array::from_fn(|k| [l[k][0] / l[k][2], l[k][1] / l[k][2]])).collect())
}

/// Port of `_weighted_median`.
pub fn weighted_median(values: &[f64], weights: &[f64]) -> f64 {
    let order = argsort(values);
    let sorted_weights: Vec<f64> = order.iter().map(|&i| weights[i]).collect();
    let mut cumulative = Vec::with_capacity(values.len());
    let mut running = 0.0;
    for &w in &sorted_weights {
        running += w;
        cumulative.push(running);
    }
    let half = pairwise_f64(&sorted_weights) / 2.0;
    let index = searchsorted(&cumulative, half, false).min(values.len().saturating_sub(1));
    values[order[index]]
}

fn directions(lines: &[Line]) -> Vec<f64> {
    lines.iter().map(|l| (l[1][1] - l[0][1]).atan2(l[1][0] - l[0][0]).to_degrees()).collect()
}

fn select<T: Copy>(values: &[T], mask: &[bool]) -> Vec<T> {
    values.iter().zip(mask).filter(|(_, &m)| m).map(|(&v, _)| v).collect()
}

fn bin_edges() -> Vec<f64> {
    // np.arange(-25.25, 25.75, .5): start + i*delta.
    (0..102).map(|i| -25.25 + f64::from(i) * 0.5).collect()
}

/// Port of `_roll`: the tilt that horizontal and vertical lines agree on, and the share of line length that
/// agrees.
pub fn roll(lines: &[Line], lengths: &[f64]) -> Result<(f64, f64)> {
    let residual: Vec<f64> = directions(lines).iter().map(|a| py_mod(a + 45.0, 90.0) - 45.0).collect();
    let keep: Vec<bool> = residual.iter().map(|r| r.abs() <= 25.0).collect();
    if keep.iter().filter(|&&k| k).count() < 3 {
        return alignment("수평·수직 기준선이 부족합니다. 네 모서리 가이드를 사용해 주세요.");
    }
    let values = select(&residual, &keep);
    let weights = select(lengths, &keep);
    let edges = bin_edges();
    let hist = histogram(&values, &weights, &edges);
    let peak = argmax(&convolve_ones5(&hist));
    let center = (edges[peak] + edges[peak + 1]) / 2.0;
    let inliers: Vec<bool> = values.iter().map(|v| (v - center).abs() < 2.0).collect();
    let total = pairwise_f64(&weights);
    let agreeing = masked_sum(&weights, &inliers);
    if inliers.iter().filter(|&&k| k).count() < 3 || agreeing < total * 0.3 {
        return alignment("일관된 수평선을 찾지 못했습니다. 가이드를 사용해 주세요.");
    }
    let angle = weighted_median(&select(&values, &inliers), &select(&weights, &inliers));
    Ok((angle, agreeing / total))
}

fn level_angles(lines: &[Line]) -> Vec<f64> {
    directions(lines).iter().map(|a| py_mod(a + 90.0, 180.0) - 90.0).collect()
}

fn middles(lines: &[Line]) -> Vec<[f64; 2]> {
    lines.iter().map(|l| [(l[0][0] + l[1][0]) / 2.0, (l[0][1] + l[1][1]) / 2.0]).collect()
}

/// Port of `_horizon_roll`: the tilt of one broad, straight horizontal boundary, and its share of line length.
pub fn horizon_roll(lines: &[Line], lengths: &[f64], width: usize, height: usize) -> Result<(f64, f64)> {
    let (w, h) = (width as f64, height as f64);
    let angle = level_angles(lines);
    let middle = middles(lines);
    let keep: Vec<bool> = (0..lines.len())
        .map(|i| {
            (lines[i][1][0] - lines[i][0][0]).abs() >= w * 0.45
                && angle[i].abs() <= 25.0
                && middle[i][1] > h * 0.15
                && middle[i][1] < h * 0.85
        })
        .collect();
    if !keep.iter().any(|&k| k) {
        return alignment("뚜렷한 수평 기준선을 찾지 못했습니다. 아래 수평 슬라이더로 조절해 주세요.");
    }
    let values = select(&angle, &keep);
    let weights = select(lengths, &keep);
    let result = weighted_median(&values, &weights);
    if values.iter().any(|v| (v - result).abs() > 1.5) {
        return alignment("긴 기준선들의 방향이 서로 다릅니다. 아래 수평 슬라이더로 조절해 주세요.");
    }
    Ok((result, pairwise_f64(&weights) / pairwise_f64(lengths)))
}

/// One result of [`plumb_rolls`] (the oracle's dict).
#[derive(Clone, Debug, PartialEq)]
pub struct Plumb {
    /// Tilt in degrees.
    pub angle: f64,
    /// Agreeing weight times (.5 + reach).
    pub score: f64,
    /// Share of upright line length that agrees.
    pub share: f64,
    /// Number of agreeing segments.
    pub count: usize,
    /// Agreeing length in frame heights.
    pub heights: f64,
    /// The agreeing edges are not all near one side of the frame.
    pub centred: bool,
    /// Agreeing edges on both sides of the centre.
    pub both: bool,
}

fn sign(v: f64) -> f64 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// Weighted least squares of `y ≈ b0 + b1·x`: `b0`.
fn weighted_intercept(x: &[f64], y: &[f64], w: &[f64]) -> f64 {
    let total: f64 = w.iter().sum();
    let mx = x.iter().zip(w).map(|(a, b)| a * b).sum::<f64>() / total;
    let my = y.iter().zip(w).map(|(a, b)| a * b).sum::<f64>() / total;
    let mut sxy = 0.0;
    let mut sxx = 0.0;
    for i in 0..x.len() {
        sxy += w[i] * (x[i] - mx) * (y[i] - my);
        sxx += w[i] * (x[i] - mx) * (x[i] - mx);
    }
    let slope = if sxx != 0.0 { sxy / sxx } else { 0.0 };
    my - slope * mx
}

/// Port of `_plumb_rolls`: tilts that upright edges support, best first (at most five).
pub fn plumb_rolls(lines: &[Line], lengths: &[f64], width: usize, height: usize) -> Vec<Plumb> {
    let direction = directions(lines);
    let upright: Vec<bool> = direction.iter().map(|d| (py_mod(d + 90.0, 180.0) - 90.0).abs() >= 65.0).collect();
    if upright.iter().filter(|&&u| u).count() < 2 {
        return Vec::new();
    }
    let residual: Vec<f64> = select(&direction, &upright).iter().map(|d| py_mod(d + 45.0, 90.0) - 45.0).collect();
    let lengths = select(lengths, &upright);
    let w = width as f64;
    let x: Vec<f64> = select(&middles(lines), &upright).iter().map(|m| (m[0] - (w - 1.0) / 2.0) / (w / 2.0)).collect();
    let weights: Vec<f64> = lengths.iter().zip(&x).map(|(l, x)| l * (1.0 - 0.5 * x.abs().min(1.0))).collect();
    let n = x.len();
    let mut candidates: Vec<(f64, f64)> = residual.iter().map(|&r| (r, 0.0)).collect();
    for i in 0..n {
        for j in i + 1..n {
            if x[i] * x[j] >= 0.0 || (x[i] - x[j]).abs() < 0.3 {
                continue;
            }
            let slope = (residual[i] - residual[j]) / (x[i] - x[j]);
            if slope.abs() <= 12.0 {
                candidates.push((residual[i] - slope * x[i], slope));
            }
        }
    }
    let mut found: Vec<(f64, f64, f64, Vec<bool>)> = Vec::new();
    for &(level, slope) in &candidates {
        if level.abs() > 20.0 {
            continue;
        }
        let inliers: Vec<bool> = (0..n).map(|i| (residual[i] - (level + slope * x[i])).abs() < 1.5).collect();
        if inliers.iter().filter(|&&k| k).count() < 2 {
            continue;
        }
        let left_mask: Vec<bool> = (0..n).map(|i| inliers[i] && x[i] < 0.0).collect();
        let right_mask: Vec<bool> = (0..n).map(|i| inliers[i] && x[i] >= 0.0).collect();
        let (left, right) = (masked_sum(&weights, &left_mask), masked_sum(&weights, &right_mask));
        let smaller = if right < left { right } else { left };
        if slope != 0.0 && smaller < 0.2 * (left + right) {
            continue;
        }
        let reach = ptp(&select(&x, &inliers)) / 2.0;
        found.push((masked_sum(&weights, &inliers) * (0.5 + reach), level, slope, inliers));
    }
    // sorted(found, key=-score): stable.
    found.sort_by(|a, b| (-a.0).partial_cmp(&-b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut results: Vec<Plumb> = Vec::new();
    for (score, _level, slope, inliers) in found {
        let xs = select(&x, &inliers);
        let rs = select(&residual, &inliers);
        let ws = select(&weights, &inliers);
        let angle = if slope != 0.0 { weighted_intercept(&xs, &rs, &ws) } else { weighted_median(&rs, &ws) };
        if results.iter().any(|r| (angle - r.angle).abs() < 1.0) {
            continue;
        }
        let signs: Vec<f64> = xs.iter().map(|&v| sign(v)).collect();
        let both = ptp(&signs) > 0.0;
        let min_abs = xs.iter().map(|v| v.abs()).fold(f64::INFINITY, f64::min);
        let agreeing = masked_sum(&lengths, &inliers);
        results.push(Plumb {
            angle,
            score,
            share: agreeing / pairwise_f64(&lengths),
            count: xs.len(),
            heights: agreeing / height as f64,
            centred: min_abs < 0.5 || both,
            both,
        });
        if results.len() == 5 {
            break;
        }
    }
    results
}

/// Port of `_wide_roll`: the tilt of level edges that agree across most of the frame's width, and their share
/// of level line length; `None` without such edges.
pub fn wide_roll(lines: &[Line], lengths: &[f64], width: usize, height: usize) -> Option<(f64, f64)> {
    let angle = level_angles(lines);
    let level: Vec<bool> = angle.iter().map(|a| a.abs() <= 25.0).collect();
    if level.iter().filter(|&&k| k).count() < 3 {
        return None;
    }
    let values = select(&angle, &level);
    let weights = select(lengths, &level);
    let points = select(lines, &level);
    let edges = bin_edges();
    let smooth = convolve_ones5(&histogram(&values, &weights, &edges));
    // np.argsort(smooth)[::-1][:12]
    let mut order = argsort(&smooth);
    order.reverse();
    let (w, h) = (width as f64, height as f64);
    let mut best: Option<(f64, Vec<bool>)> = None;
    for &peak in order.iter().take(12) {
        if smooth[peak] <= 0.0 {
            break;
        }
        let center = (edges[peak] + edges[peak + 1]) / 2.0;
        let inliers: Vec<bool> = values.iter().map(|v| (v - center).abs() < 1.5).collect();
        if inliers.iter().filter(|&&k| k).count() < 3 {
            continue;
        }
        let chosen = select(&points, &inliers);
        let mut covered = [false; 64];
        for l in &chosen {
            let (low, high) = if l[1][0] < l[0][0] { (l[1][0], l[0][0]) } else { (l[0][0], l[1][0]) };
            // covered[int(low/width*64):int(np.ceil(high/width*64))] = True, with Python slice semantics.
            let start = python_index((low / w * 64.0) as i64);
            let stop = python_index((high / w * 64.0).ceil() as i64);
            for slot in covered.iter_mut().take(stop).skip(start) {
                *slot = true;
            }
        }
        let reach = covered.iter().filter(|&&c| c).count() as f64 / 64.0;
        let rows = ptp(&chosen.iter().map(|l| (l[0][1] + l[1][1]) / 2.0).collect::<Vec<_>>()) / h;
        if reach < 0.6 || rows < 0.08 {
            continue;
        }
        let score = masked_sum(&weights, &inliers) * reach;
        if best.as_ref().is_none_or(|b| score > b.0) {
            best = Some((score, inliers));
        }
    }
    let (_, inliers) = best?;
    let angle = weighted_median(&select(&values, &inliers), &select(&weights, &inliers));
    Some((angle, masked_sum(&weights, &inliers) / pairwise_f64(&weights)))
}

/// A Python slice bound on a 64-element list.
fn python_index(i: i64) -> usize {
    if i < 0 {
        (i + 64).max(0) as usize
    } else {
        (i as usize).min(64)
    }
}

/// Port of `_level`: `(angle, confidence, algorithm)` for the Level mode.
pub fn level(lines: &[Line], lengths: &[f64], width: usize, height: usize) -> Result<(f64, f64, &'static str)> {
    let plumbs = plumb_rolls(lines, lengths, width, height);
    let horizon = horizon_roll(lines, lengths, width, height).ok();
    let wide = wide_roll(lines, lengths, width, height);
    let levels: Vec<((f64, f64), &'static str)> = [(horizon, "lines-plumb-horizon-v3"), (wide, "lines-plumb-wide-v3")]
        .into_iter()
        .filter_map(|(l, name)| l.map(|l| (l, name)))
        .collect();
    let max = |a: f64, b: f64| if b > a { b } else { a };
    let best = plumbs.first();
    let strong = best.is_some_and(|b| b.count >= 4 && b.heights >= 0.5 && b.share >= 0.5 && b.centred);
    if let (true, Some(best)) = (strong, best) {
        for &(level, name) in &levels {
            if (best.angle - level.0).abs() <= 1.5 && level.0.abs() <= 15.0 {
                return Ok(((best.angle + level.0) / 2.0, max(level.1, best.share), name));
            }
        }
        if best.angle.abs() <= 2.0 {
            return Ok((best.angle, best.share, "lines-plumb-v3"));
        }
    } else {
        for &(level, name) in &levels {
            for plumb in &plumbs {
                let best_score = best.map_or(0.0, |b| b.score);
                if (plumb.angle - level.0).abs() <= 1.5 && plumb.score >= 0.5 * best_score && level.0.abs() <= 6.0 {
                    return Ok(((plumb.angle + level.0) / 2.0, max(level.1, plumb.share), name));
                }
            }
        }
        let contradicted = match (horizon, best) {
            (Some(h), Some(b)) => b.count >= 3 && (b.angle - h.0).abs() > 3.0,
            _ => false,
        };
        if let Some(h) = horizon {
            if !contradicted && (h.0.abs() <= 4.0 || h.1 >= 0.7) {
                return Ok((h.0, h.1, "lines-horizon-v2"));
            }
        }
    }
    alignment("믿을 만한 수평·수직 기준선을 찾지 못했습니다. 아래 수평 슬라이더로 조절해 주세요.")
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// `np.linalg.norm` of a 1-D vector (`sqrt(dot(x, x))`, FMA-accumulated).
fn norm3(v: [f64; 3]) -> f64 {
    dot3(v, v).sqrt()
}

/// The right singular vector of the smallest singular value of an N×3 matrix (`np.linalg.svd(a)[2][-1]`, up
/// to sign), by one-sided Jacobi rotations.
fn smallest_right_singular(rows: &[[f64; 3]]) -> [f64; 3] {
    let mut a: Vec<[f64; 3]> = rows.to_vec();
    let mut v = EYE;
    for _sweep in 0..60 {
        let mut rotated = false;
        for p in 0..2 {
            for q in p + 1..3 {
                let (mut alpha, mut beta, mut gamma) = (0.0, 0.0, 0.0);
                for r in &a {
                    alpha += r[p] * r[p];
                    beta += r[q] * r[q];
                    gamma += r[p] * r[q];
                }
                if gamma == 0.0 || gamma.abs() <= 1e-16 * (alpha * beta).sqrt() {
                    continue;
                }
                rotated = true;
                let zeta = (beta - alpha) / (2.0 * gamma);
                let t = zeta.signum() / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let t = if zeta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = c * t;
                for r in a.iter_mut() {
                    let (x, y) = (r[p], r[q]);
                    r[p] = c * x - s * y;
                    r[q] = s * x + c * y;
                }
                for r in v.iter_mut() {
                    let (x, y) = (r[p], r[q]);
                    r[p] = c * x - s * y;
                    r[q] = s * x + c * y;
                }
            }
        }
        if !rotated {
            break;
        }
    }
    let norms: Vec<f64> = (0..3).map(|k| a.iter().map(|r| r[k] * r[k]).sum::<f64>()).collect();
    let mut k = 0;
    for i in 1..3 {
        if norms[i] < norms[k] {
            k = i;
        }
    }
    [v[0][k], v[1][k], v[2][k]]
}

/// Port of `_vanishing`: the vanishing point (homogeneous, unit length, sign as found) that most of the lines
/// meet, the number of lines that agree and their share of the weight.
pub fn vanishing(lines: &[Line], lengths: &[f64], vertical: bool) -> Result<([f64; 3], usize, f64)> {
    let n = lines.len();
    if n < 3 {
        return alignment("원근을 계산할 기준선이 부족합니다.");
    }
    let equations: Vec<[f64; 3]> = lines
        .iter()
        .map(|l| {
            let e = cross([l[0][0], l[0][1], 1.0], [l[1][0], l[1][1], 1.0]);
            let d = (e[0] * e[0] + e[1] * e[1]).sqrt();
            [e[0] / d, e[1] / d, e[2] / d]
        })
        .collect();
    let mid = middles(lines);
    let axis = if vertical { 0 } else { 1 };
    if ptp(&mid.iter().map(|m| m[axis]).collect::<Vec<_>>()) < 0.25 {
        return alignment("화면의 여러 위치에서 기준선이 필요합니다.");
    }
    let longest = lengths.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let weights: Vec<f64> = lengths.iter().map(|l| l / longest).collect();
    let mut pairs: Vec<(usize, usize)> = Vec::with_capacity(n * (n - 1) / 2);
    for i in 0..n {
        for j in i + 1..n {
            pairs.push((i, j));
        }
    }
    if pairs.len() > 500 {
        pairs = rng::choice(42, pairs.len(), 500).into_iter().map(|k| pairs[k as usize]).collect();
    }
    let threshold = 1.3f64.to_radians().sin();
    let errors = |point: [f64; 3], rows: &[usize]| -> Vec<f64> {
        rows.iter()
            .map(|&k| {
                let dx = point[0] - mid[k][0] * point[2];
                let dy = point[1] - mid[k][1] * point[2];
                let den = (dx * dx + dy * dy).sqrt();
                dot3(equations[k], point).abs() / if den > 1e-9 { den } else { 1e-9 }
            })
            .collect()
    };
    let all: Vec<usize> = (0..n).collect();
    let mut best: Option<Vec<bool>> = None;
    let mut score = -1.0;
    for &(i, j) in &pairs {
        let mut point = cross(equations[i], equations[j]);
        let norm = norm3(point);
        if norm < 1e-9 {
            continue;
        }
        point = [point[0] / norm, point[1] / norm, point[2] / norm];
        let keep: Vec<bool> = errors(point, &all).iter().map(|&e| e < threshold).collect();
        let value = masked_sum(&weights, &keep);
        if value > score {
            score = value;
            best = Some(keep);
        }
    }
    let total = pairwise_f64(&weights);
    let Some(best) = best.filter(|b| b.iter().filter(|&&k| k).count() >= 3 && masked_sum(&weights, b) >= total * 0.4)
    else {
        return alignment("같은 방향으로 모이는 직선을 찾지 못했습니다.");
    };
    let chosen: Vec<usize> = (0..n).filter(|&k| best[k]).collect();
    if ptp(&chosen.iter().map(|&k| mid[k][axis]).collect::<Vec<_>>()) < 0.25 {
        return alignment("기준선이 한쪽에 몰려 있어 안정적으로 보정할 수 없습니다.");
    }
    let rows: Vec<[f64; 3]> = chosen
        .iter()
        .map(|&k| {
            let s = weights[k].sqrt();
            [equations[k][0] * s, equations[k][1] * s, equations[k][2] * s]
        })
        .collect();
    let point = smallest_right_singular(&rows);
    if median(&errors(point, &chosen)) > 1.5f64.to_radians().sin() {
        return alignment("원근 추정의 오차가 큽니다.");
    }
    Ok((point, chosen.len(), masked_sum(&weights, &best) / total))
}

/// Port of `_crop_scale`: the zoom that keeps the corrected frame free of empty corners (1 when none is needed).
pub fn crop_scale(matrix: &M3, width: usize, height: usize) -> Result<f64> {
    let (w, h) = (width as f64, height as f64);
    let corners = [[0.0, 0.0], [w - 1.0, 0.0], [w - 1.0, h - 1.0], [0.0, h - 1.0]];
    let center = [(w - 1.0) / 2.0, (h - 1.0) / 2.0];
    let inverse = inv(matrix).ok_or_else(|| Error::Value("Singular matrix".into()))?;
    let fits = |zoom: f64| -> Result<bool> {
        let shifted: Vec<Line> = corners
            .chunks(2)
            .map(|pair| {
                std::array::from_fn(|k| {
                    [(pair[k][0] - center[0]) / zoom + center[0], (pair[k][1] - center[1]) / zoom + center[1]]
                })
            })
            .collect();
        let p = points(&inverse, &shifted)?;
        Ok(p.iter()
            .flatten()
            .all(|q| q[0] >= -0.001 && q[1] >= -0.001 && q[0] <= w - 1.0 + 0.001 && q[1] <= h - 1.0 + 0.001))
    };
    if fits(1.0)? {
        return Ok(1.0);
    }
    if !fits(2.5)? {
        return alignment("보정 후 잘리는 영역이 너무 큽니다. 수동 원근이나 가이드를 사용해 주세요.");
    }
    let (mut lo, mut hi) = (1.0, 2.5);
    for _ in 0..30 {
        let mid = (lo + hi) / 2.0;
        if fits(mid)? {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Ok(hi * 1.001)
}

/// The result of [`estimate_from_lines`] (the oracle's dict, stored as `settings['upright']`).
#[derive(Clone, Debug, PartialEq)]
pub struct Estimate {
    /// The correction in coordinates normalised to 0..1 (`n @ matrix @ inv(n)`).
    pub matrix: M3,
    /// Zoom that hides empty corners.
    pub crop_scale: f64,
    /// The mode that was used (`level`, `vertical`, `full`).
    pub mode: String,
    /// The mode that was asked for.
    pub requested_mode: String,
    /// Tilt in degrees.
    pub rotation: f64,
    /// Segments used.
    pub lines: usize,
    /// Segments that agree on the perspective.
    pub inliers: usize,
    /// Confidence 0..1.
    pub confidence: f64,
    /// Which evidence decided.
    pub algorithm: String,
}

impl Estimate {
    /// The oracle's dict as JSON (what `settings['upright']` stores).
    pub fn to_json(&self) -> Value {
        json!({
            "matrix": self.matrix, "crop_scale": self.crop_scale, "mode": self.mode,
            "requested_mode": self.requested_mode, "rotation": self.rotation, "lines": self.lines,
            "inliers": self.inliers, "confidence": self.confidence, "algorithm": self.algorithm,
        })
    }
}

/// Port of luma/upright.py `estimate` after its line detector. `width`×`height` is the image size, `lines` the
/// LSD segments (N×4 float32 `x0 y0 x1 y1`, `None` when the detector found none), `mode` one of `auto`,
/// `level`, `vertical`, `full`.
pub fn estimate_from_lines(
    width: usize,
    height: usize,
    lines: Option<ArrayView2<'_, f32>>,
    mode: &str,
) -> Result<Estimate> {
    if !matches!(mode, "level" | "vertical" | "full" | "auto") {
        return Err(Error::Value("알 수 없는 자동 정렬 모드입니다.".into()));
    }
    let (w, h) = (width, height);
    if h.min(w) < 40 {
        return alignment("사진이 너무 작아 자동 정렬을 계산할 수 없습니다.");
    }
    let Some(found) = lines else {
        return alignment("사진에서 직선 기준을 찾지 못했습니다.");
    };
    if found.ncols() != 4 {
        return Err(Error::Value("line segments must be N×4".into()));
    }
    let mut segments: Vec<Line> = found
        .rows()
        .into_iter()
        .map(|r| [[f64::from(r[0]), f64::from(r[1])], [f64::from(r[2]), f64::from(r[3])]])
        .collect();
    let mut lengths: Vec<f64> = segments
        .iter()
        .map(|l| {
            let (dx, dy) = (l[1][0] - l[0][0], l[1][1] - l[0][1]);
            (dx * dx + dy * dy).sqrt()
        })
        .collect();
    let shortest = {
        let m = h.min(w) as f64 * 0.06;
        if m > 20.0 {
            m
        } else {
            20.0
        }
    };
    let keep: Vec<bool> = lengths.iter().map(|&l| l >= shortest).collect();
    segments = select(&segments, &keep);
    lengths = select(&lengths, &keep);
    if segments.len() > 160 {
        let order = argsort(&lengths);
        let top = &order[order.len() - 160..];
        segments = top.iter().map(|&i| segments[i]).collect();
        lengths = top.iter().map(|&i| lengths[i]).collect();
    }
    let lines = segments;
    let mut algorithm = "lines-v1";
    let (angle, mut confidence) = if mode == "level" {
        let (a, c, name) = level(&lines, &lengths, w, h)?;
        algorithm = name;
        (a, c)
    } else {
        match roll(&lines, &lengths) {
            Ok(r) => r,
            Err(e) => {
                if mode != "auto" {
                    return Err(e);
                }
                algorithm = "lines-horizon-v2";
                horizon_roll(&lines, &lengths, w, h)?
            }
        }
    };
    let (wf, hf) = (w as f64, h as f64);
    let center = ((wf - 1.0) / 2.0, (hf - 1.0) / 2.0);
    let r2 = get_rotation_matrix_2d(center, angle, 1.0)?;
    let rotation: M3 = [r2[0], r2[1], [0.0, 0.0, 1.0]];
    let scale = wf.max(hf) / 2.0;
    let normal: M3 = [[1.0 / scale, 0.0, -center.0 / scale], [0.0, 1.0 / scale, -center.1 / scale], [0.0, 0.0, 1.0]];
    let leveled = points(&matmul(&normal, &rotation), &lines)?;
    let deltas: Vec<[f64; 2]> = leveled.iter().map(|l| [l[1][0] - l[0][0], l[1][1] - l[0][1]]).collect();
    let vertical: Vec<bool> = deltas.iter().map(|d| d[1].abs() > d[0].abs() * 1.5).collect();
    let horizontal: Vec<bool> = deltas.iter().map(|d| d[0].abs() > d[1].abs() * 1.5).collect();
    let mut perspective = EYE;
    let mut support = 0usize;
    let mut chosen = mode;
    let mut vertical_fallback: Option<(M3, usize, f64)> = None;
    let min = |a: f64, b: f64| if b < a { b } else { a };
    if mode != "level" {
        let attempt = (|| -> Result<()> {
            let (vp_v, count_v, conf_v) = vanishing(&select(&leveled, &vertical), &select(&lengths, &vertical), true)?;
            if vp_v[1].abs() < 0.2 {
                return alignment("수직 소실점을 안정적으로 계산할 수 없습니다.");
            }
            perspective[2][1] = -vp_v[2] / vp_v[1];
            perspective[0][1] = -vp_v[0] / vp_v[1];
            support = count_v;
            confidence = min(confidence, conf_v);
            chosen = "vertical";
            vertical_fallback = Some((perspective, support, confidence));
            if mode == "full" || mode == "auto" {
                let inner = (|| -> Result<(M3, usize, f64)> {
                    let (vp_h, count_h, conf_h) =
                        vanishing(&select(&leveled, &horizontal), &select(&lengths, &horizontal), false)?;
                    let horizon = cross(vp_h, vp_v);
                    if horizon[2].abs() < 1e-5 {
                        return alignment("소실선이 사진에 너무 가깝습니다.");
                    }
                    let mut projective = EYE;
                    projective[2] = [horizon[0] / horizon[2], horizon[1] / horizon[2], 1.0];
                    let pu = matvec(&projective, vp_h);
                    let pv = matvec(&projective, vp_v);
                    let nu = pu[1].mul_add(pu[1], pu[0] * pu[0]).sqrt();
                    let nv = pv[1].mul_add(pv[1], pv[0] * pv[0]).sqrt();
                    let mut u = [pu[0] / nu, pu[1] / nu];
                    let mut v = [pv[0] / nv, pv[1] / nv];
                    if u[0] < 0.0 {
                        u = [-u[0], -u[1]];
                    }
                    if v[1] < 0.0 {
                        v = [-v[0], -v[1]];
                    }
                    let axes = [[u[0], v[0]], [u[1], v[1]]];
                    if det(&axes).abs() < 0.3 {
                        return alignment("두 기준 방향을 분리하기 어렵습니다.");
                    }
                    let inverse = inv(&axes).ok_or_else(|| Error::Value("Singular matrix".into()))?;
                    let mut affine = EYE;
                    affine[0] = [inverse[0][0], inverse[0][1], 0.0];
                    affine[1] = [inverse[1][0], inverse[1][1], 0.0];
                    Ok((matmul(&affine, &projective), count_h, conf_h))
                })();
                match inner {
                    Ok((p, count_h, conf_h)) => {
                        perspective = p;
                        support += count_h;
                        confidence = min(confidence, conf_h);
                        chosen = "full";
                    }
                    Err(e @ Error::Alignment(_)) => {
                        if mode == "full" {
                            return Err(e);
                        }
                    }
                    Err(e) => return Err(e),
                }
            }
            Ok(())
        })();
        match attempt {
            Ok(()) => {}
            Err(e @ Error::Alignment(_)) => {
                if mode != "auto" {
                    return Err(e);
                }
                perspective = EYE;
                chosen = "level";
            }
            Err(e) => return Err(e),
        }
    }
    let normal_inv = inv(&normal).ok_or_else(|| Error::Value("Singular matrix".into()))?;
    let compose = |p: &M3| matmul(&matmul(&matmul(&normal_inv, p), &normal), &rotation);
    let mut matrix = compose(&perspective);
    let excessive = |m: &M3| crop_scale(m, w, h).map_or(true, |z| z > 1.4);
    if mode == "auto" && chosen == "full" && excessive(&matrix) {
        if let Some((p, s, c)) = vertical_fallback {
            perspective = p;
            support = s;
            confidence = c;
        }
        chosen = "vertical";
        matrix = compose(&perspective);
    }
    if mode == "auto" && chosen == "vertical" && excessive(&matrix) {
        matrix = rotation;
        chosen = "level";
        support = 0;
    }
    let corners = [[0.0, 0.0, 1.0], [wf - 1.0, 0.0, 1.0], [wf - 1.0, hf - 1.0, 1.0], [0.0, hf - 1.0, 1.0]];
    if corners.iter().any(|c| {
        let d = dot3(*c, matrix[2]);
        !(0.3..=3.0).contains(&d)
    }) {
        return alignment("원근 보정이 지나치게 큽니다. 수동 가이드를 사용해 주세요.");
    }
    let zoom = crop_scale(&matrix, w, h)?;
    let n: M3 = [[1.0 / (wf - 1.0), 0.0, 0.0], [0.0, 1.0 / (hf - 1.0), 0.0], [0.0, 0.0, 1.0]];
    let n_inv = inv(&n).ok_or_else(|| Error::Value("Singular matrix".into()))?;
    Ok(Estimate {
        matrix: matmul(&matmul(&n, &matrix), &n_inv),
        crop_scale: zoom,
        mode: chosen.to_owned(),
        requested_mode: mode.to_owned(),
        rotation: angle,
        lines: lines.len(),
        inliers: support,
        confidence,
        algorithm: algorithm.to_owned(),
    })
}

fn stored_matrix(value: &Value) -> Option<M3> {
    let rows = value.as_array()?;
    if rows.len() != 3 {
        return None;
    }
    let mut m = [[0.0; 3]; 3];
    for (slot, row) in m.iter_mut().zip(rows) {
        let row = row.as_array()?;
        if row.len() != 3 {
            return None;
        }
        for (s, v) in slot.iter_mut().zip(row) {
            *s = match v {
                Value::Number(n) => n.as_f64()?,
                Value::Bool(b) => f64::from(u8::from(*b)),
                Value::Null => f64::NAN,
                _ => return None,
            };
        }
    }
    Some(m)
}

/// Port of luma/upright.py `apply`: the stored correction (`settings['upright']`) through `cv2.warpPerspective`
/// (bilinear, zero border), zoomed by its `crop_scale` when `upright_crop`. Returns the input borrowed when no
/// correction is stored. A matrix that is not 3×3 numbers counts as the oracle's invalid-matrix error.
pub fn apply<'a>(image: &'a Array3<f32>, settings: &Settings) -> Result<Cow<'a, Array3<f32>>> {
    let data = settings.value("upright");
    if !grainy_core::settings::truthy(data) {
        return Ok(Cow::Borrowed(image));
    }
    let bad_matrix = || Error::Value("자동 원근 보정 행렬이 올바르지 않습니다.".into());
    let matrix = data.get("matrix").and_then(stored_matrix).ok_or_else(bad_matrix)?;
    if !matrix.iter().flatten().all(|v| v.is_finite()) || det(&matrix).abs() < 1e-8 {
        return Err(bad_matrix());
    }
    let zoom = if settings.bool("upright_crop") {
        match data.get("crop_scale") {
            None => 1.0,
            Some(Value::Number(n)) => n.as_f64().unwrap_or(f64::NAN),
            Some(Value::Bool(b)) => f64::from(u8::from(*b)),
            Some(_) => f64::NAN,
        }
    } else {
        1.0
    };
    if !zoom.is_finite() || !(1.0..=2.6).contains(&zoom) {
        return Err(Error::Value("자동 원근 확대 비율이 올바르지 않습니다.".into()));
    }
    let (h, w, _) = image.dim();
    let n: M3 = [[(w.max(2) - 1) as f64, 0.0, 0.0], [0.0, (h.max(2) - 1) as f64, 0.0], [0.0, 0.0, 1.0]];
    let n_inv = inv(&n).ok_or_else(bad_matrix)?;
    let matrix = matmul(&matmul(&n, &matrix), &n_inv);
    let center = [(w as f64 - 1.0) / 2.0, (h as f64 - 1.0) / 2.0];
    let crop: M3 = [[zoom, 0.0, center[0] * (1.0 - zoom)], [0.0, zoom, center[1] * (1.0 - zoom)], [0.0, 0.0, 1.0]];
    let result =
        warp_perspective(image, matmul(&crop, &matrix), (w, h), Interpolation::Linear, Border::Constant(0.0), false)?;
    Ok(Cow::Owned(result))
}
