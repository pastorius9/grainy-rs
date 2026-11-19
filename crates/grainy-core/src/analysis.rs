//! Shared analysis mathematics from luma/workflow_data.py.
use crate::numpy::float32::{exp as np_exp, ln as np_ln};
use crate::numpy::{percentile, percentiles_f64};
use crate::Image;
use ndarray::Axis;
/// The statistics of `automatic_white_balance` with numpy's dtypes: float32 pixels, float64 luminance, a float32
/// chroma threshold (`np.percentile` with a scalar), numpy's float32 `log`/`exp` (its AVX2 code, as the oracle's
/// machine runs it) and C's `log2f`.
pub fn white_balance_statistics(frame: &Image) -> std::result::Result<(f64, f64), String> {
    if frame.dim().2 != 3 {
        return Err("white balance needs three channels".into());
    }
    let mut pixels: Vec<[f32; 3]> = Vec::new();
    for p in frame.lanes(Axis(2)) {
        let rgb = [0, 1, 2].map(|c| if p[c].is_nan() { p[c] } else { p[c].max(0.0) });
        let nan = rgb.iter().any(|v| v.is_nan());
        let peak = if nan { f32::NAN } else { rgb[0].max(rgb[1]).max(rgb[2]) };
        let floor = if nan { f32::NAN } else { rgb[0].min(rgb[1]).min(rgb[2]) };
        if rgb.iter().all(|v| v.is_finite()) && floor > 0.005f32 && peak < 0.98f32 {
            pixels.push(rgb);
        }
    }
    if pixels.len() < 16 {
        return Err("화이트 밸런스를 계산할 유효한 색상 영역이 부족합니다.".into());
    }
    // Trim extreme luminance and prefer low-chroma surfaces. This is an explicitly statistical white balance; it
    // cannot infer scene intent.
    let weights = [0.2126f64, 0.7152, 0.0722];
    let luminance: Vec<f64> = pixels
        .iter()
        .map(|p| f64::from(p[0]) * weights[0] + f64::from(p[1]) * weights[1] + f64::from(p[2]) * weights[2])
        .collect();
    let ends = percentiles_f64(&luminance, &[10.0, 90.0]);
    let trimmed: Vec<[f32; 3]> =
        pixels.iter().zip(&luminance).filter(|(_, l)| **l >= ends[0] && **l <= ends[1]).map(|(p, _)| *p).collect();
    let chroma: Vec<f32> = trimmed
        .iter()
        .map(|p| {
            let peak = p[0].max(p[1]).max(p[2]);
            (peak - p[0].min(p[1]).min(p[2])) / peak.max(1e-6f32)
        })
        .collect();
    let threshold = percentile(&mut chroma.clone(), 40.0);
    // np.mean(axis=0) of float32 rows: a float32 running sum per channel, then a float32 division.
    let mut sum = [0f32; 3];
    let mut count = 0usize;
    for (p, c) in trimmed.iter().zip(&chroma) {
        if *c <= threshold {
            for k in 0..3 {
                sum[k] += p[k];
            }
            count += 1;
        }
    }
    let average = sum.map(|v| {
        let mean = v / count as f32;
        if mean.is_nan() {
            mean
        } else {
            mean.max(1e-6f32)
        }
    });
    let logs = average.map(np_ln);
    let mean_log = (logs[0] + logs[1] + logs[2]) / 3.0f32;
    let centre = np_exp(mean_log);
    let gains = average.map(|v| centre / v);
    let r = (gains[0] / gains[1]).log2();
    let b = (gains[2] / gains[1]).log2();
    let clip32 = |v: f32| if v.is_nan() { v } else { v.clamp(-100.0, 100.0) };
    Ok((f64::from(clip32(90.0f32 * (r - b))), f64::from(clip32((r + b) / 0.013f32))))
}
