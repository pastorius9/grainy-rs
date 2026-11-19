//! Port of `luma/auto_color.py`: automatic colour-cast correction written as RGB channel curves.
//!
//! `auto_color_settings` of the oracle develops the source (geometry, lens shading, tone) before calling
//! [`channel_points`] on the result of `_develop_color_mix`; that orchestration belongs to the engine crate.

use grainy_core::Image;

use crate::convert::{clip01, f};

/// Luminance ranges: shadows, midtones, highlights.
pub const BANDS: [(f64, f64); 3] = [(0.02, 0.25), (0.25, 0.7), (0.7, 0.98)];
/// Cluster sizes tried in turn: the smallest with enough pixels wins.
pub const RADII: [f64; 3] = [0.06, 0.12, 0.2];
/// Share of a band's pixels the neutral cluster must hold.
pub const SUPPORT: f64 = 0.25;
/// Share of the picture a band must cover to be judged.
pub const MIN_BAND: f64 = 0.02;
/// ...and to accept a wider cluster.
pub const WIDE_BAND: f64 = 0.15;
/// Largest correction of any channel (display-encoded 0-1).
pub const LIMIT: f64 = 0.2;
/// Anchors closer than this are merged into the earlier one.
pub const GAP: f64 = 0.04;
/// `WEIGHTS` (float32 Rec. 709).
pub const WEIGHTS: [f32; 3] = [f(0.2126), f(0.7152), f(0.0722)];

/// The outcome of the analysis (the oracle's report dict).
#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    /// `"applied"`, `"neutral"` or `"flat"`.
    pub status: &'static str,
    /// The amount used (only when applied).
    pub amount: Option<f64>,
    /// Per band, the measured cast (channel minus luminance, rounded to 4 decimals) or `None`; absent when the
    /// picture has too few usable pixels.
    pub casts: Option<Vec<Option<[f64; 3]>>>,
}

/// Channel curves: three lists of `[x, y]` points.
pub type Curves = [Vec<[f64; 2]>; 3];

pub(crate) use grainy_core::numpy::mean_flat as mean_1d;

/// `x.mean(axis=0)` of an N×3 float32 array (numpy adds the rows in order, without pairwise summation).
fn mean_rows(rows: &[[f32; 3]]) -> [f32; 3] {
    let mut sum = [0f32; 3];
    for row in rows {
        for c in 0..3 {
            sum[c] += row[c];
        }
    }
    sum.map(|s| (f64::from(s) / rows.len() as f64) as f32)
}

/// Port of `neutral_offset`: the offset of the near-neutral cluster from grey (channel minus luminance) and the
/// pixels in it, or `None`.
pub fn neutral_offset(chroma: &[[f32; 3]], radii: &[f64]) -> Option<([f32; 3], Vec<bool>)> {
    let n = chroma.len() as f64;
    for &radius in radii {
        let radius = radius as f32;
        let mut centre = [0f32; 3];
        let mut near: Option<Vec<bool>> = None;
        for _ in 0..5 {
            let mask: Vec<bool> = chroma
                .iter()
                .map(|p| {
                    let d = [(p[0] - centre[0]).abs(), (p[1] - centre[1]).abs(), (p[2] - centre[2]).abs()];
                    d[0].max(d[1]).max(d[2]) < radius
                })
                .collect();
            let count = mask.iter().filter(|&&x| x).count();
            if (count as f64 / n) < SUPPORT || n == 0.0 {
                near = None;
                break;
            }
            let selected: Vec<[f32; 3]> = chroma.iter().zip(&mask).filter(|(_, &m)| m).map(|(p, _)| *p).collect();
            centre = mean_rows(&selected);
            near = Some(mask);
        }
        if let Some(mask) = near {
            return Some((centre, mask));
        }
    }
    None
}

/// Python's `round(x, 4)` (correctly rounded through the decimal representation).
fn round4(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.4}").parse().unwrap_or(x)
}

/// Port of `channel_points`: (three point lists, report) for display-encoded RGB (H×W×3, 0-1) as it enters the
/// channel curves, or (`None`, report) when no band has a measurable cast. `amount` (0-1) scales every shift.
pub fn channel_points(image: &Image, amount: f64) -> (Option<Curves>, Report) {
    let mut pixels: Vec<[f32; 3]> = Vec::new();
    for p in image.lanes(ndarray::Axis(2)) {
        let px = [clip01(p[0]), clip01(p[1]), clip01(p[2])];
        if px.iter().all(|v| v.is_finite()) && px[0].max(px[1]).max(px[2]) < f(0.995) {
            pixels.push(px);
        }
    }
    if pixels.len() < 256 {
        return (None, Report { status: "flat", amount: None, casts: None });
    }
    let total = pixels.len() as f64;
    let luminance: Vec<f32> =
        pixels.iter().map(|p| p[0] * WEIGHTS[0] + p[1] * WEIGHTS[1] + p[2] * WEIGHTS[2]).collect();
    let chroma: Vec<[f32; 3]> = pixels.iter().zip(&luminance).map(|(p, &l)| [p[0] - l, p[1] - l, p[2] - l]).collect();
    let amount = if amount.is_nan() { amount } else { amount.clamp(0.0, 1.0) };
    let mut anchors: Vec<(f64, [f32; 3])> = Vec::new();
    let mut casts: Vec<Option<[f64; 3]>> = Vec::new();
    for (low, high) in BANDS {
        let (low, high) = (low as f32, high as f32);
        let inside: Vec<usize> = (0..pixels.len()).filter(|&i| luminance[i] >= low && luminance[i] < high).collect();
        let share = inside.len() as f64 / total;
        if share < MIN_BAND {
            casts.push(None);
            continue;
        }
        let band_chroma: Vec<[f32; 3]> = inside.iter().map(|&i| chroma[i]).collect();
        let radii: &[f64] = if share >= WIDE_BAND { &RADII } else { &RADII[..1] };
        let Some((offset, near)) = neutral_offset(&band_chroma, radii) else {
            casts.push(None);
            continue;
        };
        let near_luminance: Vec<f32> =
            inside.iter().zip(&near).filter(|(_, &m)| m).map(|(&i, _)| luminance[i]).collect();
        let level = f64::from(mean_1d(&near_luminance));
        let limit = LIMIT as f32;
        let offset = offset.map(|v| v.clamp(-limit, limit));
        casts.push(Some(offset.map(|v| round4(f64::from(v)))));
        if anchors.last().is_none_or(|last| level - last.0 >= GAP) {
            anchors.push((level, offset));
        }
    }
    let largest = anchors
        .iter()
        .map(|(_, o)| o.iter().fold(0f32, |m, v| m.max(v.abs())))
        .fold(f64::NEG_INFINITY, |m, v| m.max(f64::from(v)));
    if anchors.is_empty() || largest < 0.004 {
        let status = if anchors.is_empty() { "flat" } else { "neutral" };
        return (None, Report { status, amount: None, casts: Some(casts) });
    }
    let curves: Curves = std::array::from_fn(|c| {
        let mut points: Vec<[f64; 2]> = vec![[0.0, 0.0]];
        for (level, offset) in &anchors {
            let sum = *level as f32 + offset[c];
            let x = f64::from(sum.clamp((GAP / 2.0) as f32, (1.0 - GAP / 2.0) as f32));
            let y = x + (level - x) * amount;
            let last = points[points.len() - 1];
            if x - last[0] >= GAP / 2.0 {
                points.push([round4(x), round4(y.max(last[1]).clamp(0.0, 1.0))]);
            }
        }
        points.push([1.0, 1.0]);
        points
    });
    (Some(curves), Report { status: "applied", amount: Some(amount), casts: Some(casts) })
}
