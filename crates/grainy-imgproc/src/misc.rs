use crate::{array, border, Border, Error, Pixel, Result};
use ndarray::{Array, Array2, Dimension};

/// Port of cv2.absdiff in luma/dust.py, preserving input rank and f32/u8 dtype.
pub fn absdiff<T: Pixel, D: Dimension>(a: &Array<T, D>, b: &Array<T, D>) -> Result<Array<T, D>> {
    array::view(a)?;
    if a.shape() != b.shape() {
        return Err(Error::Shape("absdiff shapes must match"));
    }
    Ok(ndarray::Zip::from(a).and(b).map_collect(|&x, &y| T::from_float((x.float() - y.float()).abs())))
}

/// Port of cv2.Laplacian(..., CV_32F) in luma/processing.py (default ksize=1, scale=1, delta=0).
pub fn laplacian<D: Dimension>(src: &Array<f32, D>, border_mode: Border) -> Result<Array<f32, D>> {
    let a = array::view(src)?;
    let (h, w, c) = a.dim();
    if border_mode == Border::Wrap {
        return Err(Error::Argument("Laplacian does not support wrap border"));
    }
    let out = array::generate(h, w, c, |y, x, c| {
        let y = y as i64;
        let x = x as i64;
        // filter2D's row-major sparse kernel accumulation, including the negative center.
        let mut v = border::sample(a, y - 1, x, c, border_mode);
        v += border::sample(a, y, x - 1, c, border_mode);
        v += -4.0 * border::sample(a, y, x, c, border_mode);
        v += border::sample(a, y, x + 1, c, border_mode);
        v + border::sample(a, y + 1, x, c, border_mode)
    });
    array::finish(out, src.ndim())
}

/// Port of cv2.bilateralFilter in luma/processing.py luma_noise_reduction; float32 single-channel.
/// Uses the oracle's 4096-bin range-weight table and circular spatial support.
pub fn bilateral_filter(
    src: &Array2<f32>,
    diameter: i32,
    sigma_color: f64,
    sigma_space: f64,
    border_mode: Border,
) -> Result<Array2<f32>> {
    let a = array::view(src)?;
    let (h, w, _) = a.dim();
    if !sigma_color.is_finite() || !sigma_space.is_finite() || src.iter().any(|v| !v.is_finite()) {
        return Err(Error::Argument("bilateral filter requires finite values"));
    }
    if sigma_color <= 1e-6 || sigma_space <= 1e-6 {
        return Ok(src.clone());
    }
    let min = src.iter().copied().fold(f32::INFINITY, f32::min);
    let max = src.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if (max as f64 - min as f64).abs() < f32::EPSILON as f64 {
        return Ok(src.clone());
    }
    let radius =
        if diameter <= 0 { (sigma_space * 1.5).round_ties_even().max(1.0) } else { (diameter / 2).max(1) as f64 };
    if radius > 32767.0 {
        return Err(Error::Argument("bilateral radius exceeds 32767"));
    }
    if border_mode == Border::Reflect101 {
        return Ok(crate::exact::bilateral::bilateral_filter(src, diameter, sigma_color, sigma_space));
    }
    let radius = radius as i64;
    let scale = 4096.0 / (max - min);
    let color = -0.5 / (sigma_color * sigma_color);
    let space = -0.5 / (sigma_space * sigma_space);
    if !scale.is_finite() || scale <= 0.0 {
        return Err(Error::Argument("bilateral input dynamic range is unsupported"));
    }
    let lut: Vec<f32> = (0..4098)
        .map(|i| {
            let v = (i as f32 / scale) as f64;
            (v * v * color).exp() as f32
        })
        .collect();
    if lut.iter().any(|v| !v.is_finite()) {
        return Err(Error::Argument("bilateral weights exceed finite range"));
    }
    let mut offsets = Vec::new();
    for y in -radius..=radius {
        for x in -radius..=radius {
            let r = ((x * x + y * y) as f64).sqrt();
            if r <= radius as f64 && (x != 0 || y != 0) {
                offsets.push((y, x, (r * r * space).exp() as f32));
            }
        }
    }
    let out = array::generate(h, w, 1, |y, x, _| {
        let center = src[[y, x]];
        let (mut sum, mut weight) = (0.0_f32, 0.0_f32);
        for &(dy, dx, spatial) in &offsets {
            let v = border::sample(a, y as i64 + dy, x as i64 + dx, 0, border_mode);
            let alpha = ((v - center).abs() * scale).min(4096.0);
            let i = alpha as usize;
            let alpha = alpha - i as f32;
            let wt = spatial * (lut[i] + alpha * (lut[i + 1] - lut[i]));
            weight += wt;
            sum += v * wt;
        }
        (sum + center) / (weight + 1.0)
    });
    array::finish(out, 2)
}

/// Port of the cv2.matchTemplate modes used by luma/dust.py and dust_texture.py.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateMatch {
    /// cv2.TM_SQDIFF, unnormalized squared difference.
    SqDiff,
    /// cv2.TM_CCOEFF_NORMED, per-channel mean-centered normalized correlation.
    CCoeffNormed,
}

/// Port of cv2.matchTemplate on float32 planes/RGB images, without a mask.
/// The result is a plane of fully contained template placements. Constant templates normalize to one.
pub fn match_template<D: Dimension>(
    src: &Array<f32, D>,
    template: &Array<f32, D>,
    method: TemplateMatch,
) -> Result<Array2<f32>> {
    let mut a = array::view(src)?;
    let mut t = array::view(template)?;
    if a.dim().2 != t.dim().2 || a.dim().2 > 4 {
        return Err(Error::Shape("template channels must match, at most four"));
    }
    if (a.dim().0 < t.dim().0 || a.dim().1 < t.dim().1) && a.dim().0 <= t.dim().0 && a.dim().1 <= t.dim().1 {
        std::mem::swap(&mut a, &mut t);
    }
    let (h, w, cn) = a.dim();
    let (th, tw, _) = t.dim();
    if th > h || tw > w {
        return Err(Error::Shape("template must fit the image on both axes"));
    }
    if a.iter().chain(t.iter()).any(|v| !v.is_finite()) {
        return Err(Error::Argument("finite template values required"));
    }
    let inv = 1.0 / (th * tw) as f64;
    let mut means = vec![0.0; cn];
    let mut squares = vec![0.0; cn];
    for ((_, _, c), &v) in t.indexed_iter() {
        means[c] += v as f64;
        squares[c] += (v as f64) * (v as f64);
    }
    let mut variance = 0.0;
    for c in 0..cn {
        means[c] *= inv;
        variance += (squares[c] * inv - means[c] * means[c]).max(0.0);
    }
    let norm = variance.sqrt() / inv.sqrt();
    let total = squares.iter().sum::<f64>();
    let out = array::generate(h - th + 1, w - tw + 1, 1, |y, x, _| {
        if method == TemplateMatch::CCoeffNormed && variance < f64::EPSILON {
            return 1.0;
        }
        let mut cross = 0.0;
        let mut sums = [0.0; 4];
        let mut sq = 0.0;
        for ty in 0..th {
            for tx in 0..tw {
                for c in 0..cn {
                    let v = a[[y + ty, x + tx, c]] as f64;
                    let tv = t[[ty, tx, c]] as f64;
                    cross += v * tv;
                    sums[c] += v;
                    sq += v * v;
                }
            }
        }
        // OpenCV stores the raw cross correlation in float32 before normalization.
        let mut num = cross as f32 as f64;
        if method == TemplateMatch::SqDiff {
            return (sq - 2.0 * num + total).max(0.0) as f32;
        }
        let mut mean2 = 0.0;
        for c in 0..cn {
            mean2 += sums[c] * sums[c];
            num -= sums[c] * means[c];
        }
        let diff = (sq - mean2 * inv).max(0.0);
        let denominator = if diff <= 0.5_f64.min(10.0 * f32::EPSILON as f64 * sq) { 0.0 } else { diff.sqrt() * norm };
        if num.abs() < denominator {
            (num / denominator) as f32
        } else if num.abs() < denominator * 1.125 {
            if num > 0.0 {
                1.0
            } else {
                -1.0
            }
        } else {
            0.0
        }
    });
    array::finish(out, 2)
}

/// Port of cv2.moments spatial, central and normalized moment dictionaries.
#[derive(Debug, Clone, Default)]
pub struct Moments {
    /// cv2.moments m00.
    pub m00: f64,
    /// cv2.moments m10.
    pub m10: f64,
    /// cv2.moments m01.
    pub m01: f64,
    /// cv2.moments m20.
    pub m20: f64,
    /// cv2.moments m11.
    pub m11: f64,
    /// cv2.moments m02.
    pub m02: f64,
    /// cv2.moments m30.
    pub m30: f64,
    /// cv2.moments m21.
    pub m21: f64,
    /// cv2.moments m12.
    pub m12: f64,
    /// cv2.moments m03.
    pub m03: f64,
    /// cv2.moments mu20.
    pub mu20: f64,
    /// cv2.moments mu11.
    pub mu11: f64,
    /// cv2.moments mu02.
    pub mu02: f64,
    /// cv2.moments mu30.
    pub mu30: f64,
    /// cv2.moments mu21.
    pub mu21: f64,
    /// cv2.moments mu12.
    pub mu12: f64,
    /// cv2.moments mu03.
    pub mu03: f64,
    /// cv2.moments nu20.
    pub nu20: f64,
    /// cv2.moments nu11.
    pub nu11: f64,
    /// cv2.moments nu02.
    pub nu02: f64,
    /// cv2.moments nu30.
    pub nu30: f64,
    /// cv2.moments nu21.
    pub nu21: f64,
    /// cv2.moments nu12.
    pub nu12: f64,
    /// cv2.moments nu03.
    pub nu03: f64,
}

/// Port of cv2.moments(component, True) in luma/dust_scratches.py; also supports weighted uint8 masks.
pub fn moments(src: &Array2<u8>, binary: bool) -> Result<Moments> {
    array::view(src)?;
    let mut m = Moments::default();
    for ((y, x), &v) in src.indexed_iter() {
        let v = if binary { f64::from(v != 0) } else { v as f64 };
        let x = x as f64;
        let y = y as f64;
        m.m00 += v;
        m.m10 += x * v;
        m.m01 += y * v;
        m.m20 += x * x * v;
        m.m11 += x * y * v;
        m.m02 += y * y * v;
        m.m30 += x * x * x * v;
        m.m21 += x * x * y * v;
        m.m12 += x * y * y * v;
        m.m03 += y * y * y * v;
    }
    let inv = if m.m00.abs() > f64::EPSILON { 1.0 / m.m00 } else { 0.0 };
    let x = m.m10 * inv;
    let y = m.m01 * inv;
    m.mu20 = m.m20 - m.m10 * x;
    m.mu11 = m.m11 - m.m10 * y;
    m.mu02 = m.m02 - m.m01 * y;
    m.mu30 = m.m30 - x * (3.0 * m.mu20 + x * m.m10);
    m.mu21 = m.m21 - x * (2.0 * m.mu11 + x * m.m01) - y * m.mu20;
    m.mu12 = m.m12 - y * (2.0 * m.mu11 + y * m.m10) - x * m.mu02;
    m.mu03 = m.m03 - y * (3.0 * m.mu02 + y * m.m01);
    let s2 = inv * inv;
    let s3 = s2 * inv.abs().sqrt();
    m.nu20 = m.mu20 * s2;
    m.nu11 = m.mu11 * s2;
    m.nu02 = m.mu02 * s2;
    m.nu30 = m.mu30 * s3;
    m.nu21 = m.mu21 * s3;
    m.nu12 = m.mu12 * s3;
    m.nu03 = m.mu03 * s3;
    Ok(m)
}
