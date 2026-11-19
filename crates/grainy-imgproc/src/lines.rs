use crate::{array, border, Border, Error, Result};
use ndarray::Array2;
use std::f64::consts::PI;

/// Port of cv2.HoughLinesP in luma/sky_mask.py; LINE_8 binary input and ordered int32 endpoints.
/// Uses OpenCV's locally seeded RNG, float32 angle table and 16-bit fixed-point line walks.
pub fn hough_lines_p(
    src: &Array2<u8>,
    rho: f64,
    theta: f64,
    threshold: i32,
    min_length: f64,
    max_gap: f64,
) -> Result<Vec<[i32; 4]>> {
    let (h, w) = src.dim();
    array::shape(h, w, 1)?;
    if !rho.is_finite()
        || !theta.is_finite()
        || rho <= 0.0
        || theta <= 0.0
        || theta > PI
        || threshold <= 0
        || !min_length.is_finite()
        || !max_gap.is_finite()
        || min_length < 0.0
        || max_gap < 0.0
        || min_length > i32::MAX as f64
        || max_gap > i32::MAX as f64
    {
        return Err(Error::Argument("invalid probabilistic Hough parameters"));
    }
    let rho = rho as f32;
    let theta = theta as f32;
    if !rho.is_finite() || rho <= 0.0 || theta <= 0.0 || PI / theta as f64 > 1_000_000.0 {
        return Err(Error::Argument("Hough resolutions exceed supported float32 range"));
    }
    let inv = 1.0 / rho;
    let mut na = (PI / theta as f64).floor() as usize + 1;
    if na > 1 && (PI - (na - 1) as f64 * theta as f64).abs() < (theta as f64) / 2.0 {
        na -= 1;
    }
    let nr = (((w + h) * 2 + 1) as f32 / rho).round_ties_even() as usize;
    array::shape(na, nr, 1)?;
    if na > 1_000_000 || na.checked_mul(nr).is_none_or(|n| n > 100_000_000) {
        return Err(Error::Argument("Hough accumulator too large"));
    }
    let table: Vec<_> = (0..na)
        .map(|i| {
            let a = i as f64 * theta as f64;
            [(a.cos() * inv as f64) as f32, (a.sin() * inv as f64) as f32]
        })
        .collect();
    let mut accumulator = Array2::<i32>::zeros((na, nr));
    let mut mask = src.mapv(|v| v != 0);
    let mut points: Vec<_> =
        src.indexed_iter().filter(|(_, v)| **v != 0).map(|((y, x), _)| (y as i64, x as i64)).collect();
    let mut rng = u64::MAX;
    let length = min_length.round_ties_even() as i64;
    let gap_limit = max_gap.round_ties_even() as i64;
    let mut lines = Vec::new();
    let bin = |x: i64, y: i64, trig: [f32; 2]| -> usize {
        ((x as f32 * trig[0] + y as f32 * trig[1]).round_ties_even() as i64 + (nr as i64 - 1) / 2) as usize
    };
    while !points.is_empty() {
        rng = (rng as u32 as u64).wrapping_mul(4164903690).wrapping_add(rng >> 32);
        let index = rng as u32 as usize % points.len();
        let (y, x) = points.swap_remove(index);
        if !mask[[y as usize, x as usize]] {
            continue;
        }
        let mut maximum = threshold - 1;
        let mut angle = 0;
        for (i, &t) in table.iter().enumerate() {
            let r = bin(x, y, t);
            if r >= nr {
                return Err(Error::Argument("Hough rho resolution overflow"));
            }
            accumulator[[i, r]] += 1;
            if accumulator[[i, r]] > maximum {
                maximum = accumulator[[i, r]];
                angle = i;
            }
        }
        if maximum < threshold {
            continue;
        }
        let a = -table[angle][1];
        let b = table[angle][0];
        let horizontal = a.abs() > b.abs();
        let (x0, y0, dx, dy) = if horizontal {
            (x, (y << 16) + 32768, if a > 0.0 { 1 } else { -1 }, (b * 65536.0 / a.abs()).round_ties_even() as i64)
        } else {
            ((x << 16) + 32768, y, (a * 65536.0 / b.abs()).round_ties_even() as i64, if b > 0.0 { 1 } else { -1 })
        };
        let mut ends = [[x, y]; 2];
        for (side, end) in ends.iter_mut().enumerate() {
            let sign = if side == 0 { 1 } else { -1 };
            let (mut xx, mut yy, mut gap) = (x0, y0, 0);
            loop {
                let (ix, iy) = if horizontal { (xx, yy >> 16) } else { (xx >> 16, yy) };
                if ix < 0 || iy < 0 || ix >= w as i64 || iy >= h as i64 {
                    break;
                }
                if mask[[iy as usize, ix as usize]] {
                    gap = 0;
                    *end = [ix, iy];
                } else {
                    gap += 1;
                    if gap > gap_limit {
                        break;
                    }
                }
                xx += dx * sign;
                yy += dy * sign;
            }
        }
        let good = (ends[1][0] - ends[0][0]).abs() >= length || (ends[1][1] - ends[0][1]).abs() >= length;
        for (side, &end) in ends.iter().enumerate() {
            let sign = if side == 0 { 1 } else { -1 };
            let (mut xx, mut yy) = (x0, y0);
            loop {
                let (ix, iy) = if horizontal { (xx, yy >> 16) } else { (xx >> 16, yy) };
                if mask[[iy as usize, ix as usize]] {
                    if good {
                        for (i, &t) in table.iter().enumerate() {
                            let r = bin(ix, iy, t);
                            accumulator[[i, r]] -= 1;
                        }
                    }
                    mask[[iy as usize, ix as usize]] = false;
                }
                if [ix, iy] == end {
                    break;
                }
                xx += dx * sign;
                yy += dy * sign;
            }
        }
        if good {
            lines.push([ends[0][0] as i32, ends[0][1] as i32, ends[1][0] as i32, ends[1][1] as i32]);
        }
    }
    Ok(lines)
}

// Default LSD_REFINE_STD preprocessing is byte-exact: an 8-bit fixed-point Gaussian,
// followed by INTER_LINEAR_EXACT at the requested scale (not a recomputed size ratio).
fn preprocess(src: &Array2<u8>) -> Result<(Array2<u8>, Array2<u8>)> {
    let (h, w) = src.dim();
    let kernel = crate::get_gaussian_kernel_f64(7, 0.6 / 0.8)?;
    let mut weights = [0_i64; 7];
    let mut error = 0.0;
    let mut sum = 0;
    for i in 0..3 {
        let adjusted = kernel[i] * 256.0 + error;
        let v = adjusted.round_ties_even() as i64;
        error = adjusted - v as f64;
        weights[i] = v;
        weights[6 - i] = v;
        sum += 2 * v;
    }
    weights[3] = 256 - sum;
    let mut horizontal = Array2::<i64>::zeros((h, w));
    for y in 0..h {
        for x in 0..w {
            for (k, &v) in weights.iter().enumerate() {
                let xx = border::index(x as i64 + k as i64 - 3, w, Border::Reflect101).unwrap_or(0);
                horizontal[[y, x]] += src[[y, xx]] as i64 * v;
            }
        }
    }
    let mut blurred = Array2::<u8>::zeros((h, w));
    for y in 0..h {
        for x in 0..w {
            let mut value = 0;
            for (k, &v) in weights.iter().enumerate() {
                let yy = border::index(y as i64 + k as i64 - 3, h, Border::Reflect101).unwrap_or(0);
                value += horizontal[[yy, x]] * v;
            }
            blurred[[y, x]] = ((value + 32768) >> 16) as u8;
        }
    }
    let dh = (h as f64 * 0.8).round_ties_even() as usize;
    let dw = (w as f64 * 0.8).round_ties_even() as usize;
    let coeff = |p: usize, n: usize| {
        let q = (p as f64 + 0.5) / 0.8 - 0.5;
        let i = q.floor() as usize;
        if i >= n - 1 {
            (n - 1, n - 1, 0_i64)
        } else {
            (i, i + 1, ((q - q.floor()) * 256.0).round_ties_even() as i64)
        }
    };
    let scaled = Array2::from_shape_fn((dh, dw), |(y, x)| {
        let (x0, x1, fx) = coeff(x, w);
        let (y0, y1, fy) = coeff(y, h);
        let top = blurred[[y0, x0]] as i64 * (256 - fx) + blurred[[y0, x1]] as i64 * fx;
        let bottom = blurred[[y1, x0]] as i64 * (256 - fx) + blurred[[y1, x1]] as i64 * fx;
        ((top * (256 - fy) + bottom * fy + 32768) >> 16) as u8
    });
    Ok((blurred, scaled))
}

fn fast_atan(y: f32, x: f32) -> f64 {
    let factor = (180.0 / PI) as f32;
    let p1 = 0.999_787_87_f32 * factor;
    let p3 = -0.325_808_4_f32 * factor;
    let p5 = 0.155_578_66_f32 * factor;
    let p7 = -0.044_326_555_f32 * factor;
    let ax = x.abs();
    let ay = y.abs();
    let c = ax.min(ay) / (ax.max(ay) + f64::EPSILON as f32);
    let c2 = c * c;
    let mut angle = (((p7 * c2 + p5) * c2 + p3) * c2 + p1) * c;
    if ax < ay {
        angle = 90.0 - angle;
    }
    if x < 0.0 {
        angle = 180.0 - angle;
    }
    if y < 0.0 {
        angle = 360.0 - angle;
    }
    angle as f64
}

fn angle_diff(a: f64, b: f64) -> f64 {
    let mut d = a - b;
    while d <= -PI {
        d += 2.0 * PI;
    }
    while d > PI {
        d -= 2.0 * PI;
    }
    d
}
fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]) * (a[0] - b[0]) + (a[1] - b[1]) * (a[1] - b[1])
}
#[derive(Clone, Copy)]
struct RegionPoint {
    x: usize,
    y: usize,
    angle: f64,
    weight: f64,
}
#[derive(Clone, Copy)]
struct Rect {
    a: [f64; 2],
    b: [f64; 2],
    width: f64,
}
struct Detector {
    angles: Array2<f64>,
    gradient: Array2<f64>,
    used: Array2<bool>,
}

impl Detector {
    fn aligned(&self, x: usize, y: usize, angle: f64, prec: f64) -> bool {
        let a = self.angles[[y, x]];
        if a == -1024.0 {
            return false;
        }
        let mut d = (angle - a).abs();
        if d > 1.5 * PI {
            d = (d - 2.0 * PI).abs();
        }
        d <= prec
    }
    fn grow(&mut self, x: usize, y: usize, prec: f64) -> (Vec<RegionPoint>, f64) {
        let mut angle = self.angles[[y, x]];
        let mut region = vec![RegionPoint { x, y, angle, weight: self.gradient[[y, x]] }];
        let mut sx = angle.cos() as f32;
        let mut sy = angle.sin() as f32;
        self.used[[y, x]] = true;
        let mut i = 0;
        let (h, w) = self.used.dim();
        while i < region.len() {
            let p = region[i];
            for yy in p.y.saturating_sub(1)..=(p.y + 1).min(h - 1) {
                for xx in p.x.saturating_sub(1)..=(p.x + 1).min(w - 1) {
                    if !self.used[[yy, xx]] && self.aligned(xx, yy, angle, prec) {
                        let a = self.angles[[yy, xx]];
                        self.used[[yy, xx]] = true;
                        region.push(RegionPoint { x: xx, y: yy, angle: a, weight: self.gradient[[yy, xx]] });
                        sx += (a as f32).cos();
                        sy += (a as f32).sin();
                        angle = fast_atan(sy, sx) * (PI / 180.0);
                    }
                }
            }
            i += 1;
        }
        (region, angle)
    }
    fn rectangle(&self, region: &[RegionPoint], angle: f64, prec: f64) -> Rect {
        let (mut x, mut y, mut sum) = (0.0, 0.0, 0.0);
        for p in region {
            x += p.x as f64 * p.weight;
            y += p.y as f64 * p.weight;
            sum += p.weight;
        }
        x /= sum;
        y /= sum;
        let (mut ixx, mut iyy, mut ixy) = (0.0_f64, 0.0_f64, 0.0_f64);
        for p in region {
            let dx = p.x as f64 - x;
            let dy = p.y as f64 - y;
            ixx += dy * dy * p.weight;
            iyy += dx * dx * p.weight;
            ixy -= dx * dy * p.weight;
        }
        let lambda = 0.5 * (ixx + iyy - ((ixx - iyy) * (ixx - iyy) + 4.0 * ixy * ixy).sqrt());
        let mut theta = if ixx.abs() > iyy.abs() {
            fast_atan((lambda - ixx) as f32, ixy as f32)
        } else {
            fast_atan(ixy as f32, (lambda - iyy) as f32)
        } * (PI / 180.0);
        if angle_diff(theta, angle).abs() > prec {
            theta += PI;
        }
        let dx = theta.cos();
        let dy = theta.sin();
        let (mut lmin, mut lmax, mut wmin, mut wmax) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
        for p in region {
            let px = p.x as f64 - x;
            let py = p.y as f64 - y;
            let l = px * dx + py * dy;
            let w = -px * dy + py * dx;
            lmin = lmin.min(l);
            lmax = lmax.max(l);
            wmin = wmin.min(w);
            wmax = wmax.max(w);
        }
        Rect { a: [x + lmin * dx, y + lmin * dy], b: [x + lmax * dx, y + lmax * dy], width: (wmax - wmin).max(1.0) }
    }
    fn refine(&mut self, region: &mut Vec<RegionPoint>, prec: f64, rect: &mut Rect) -> bool {
        let density = |n: usize, r: Rect| n as f64 / (distance(r.a, r.b).sqrt() * r.width);
        if density(region.len(), *rect) >= 0.7 {
            return true;
        }
        let seed = region[0];
        let center = [seed.x as f64, seed.y as f64];
        let (mut sum, mut sq, mut n) = (0.0, 0.0, 0);
        for p in region.iter() {
            self.used[[p.y, p.x]] = false;
            if distance(center, [p.x as f64, p.y as f64]).sqrt() < rect.width {
                let d = angle_diff(p.angle, seed.angle);
                sum += d;
                sq += d * d;
                n += 1;
            }
        }
        let mean = sum / n as f64;
        let tau = 2.0 * ((sq - 2.0 * mean * sum) / n as f64 + mean * mean).sqrt();
        let (new, angle) = self.grow(seed.x, seed.y, tau);
        *region = new;
        if region.len() < 2 {
            return false;
        }
        *rect = self.rectangle(region, angle, prec);
        let mut radius = distance(center, rect.a).max(distance(center, rect.b));
        while density(region.len(), *rect) < 0.7 {
            radius *= 0.75 * 0.75;
            let mut i = 0;
            while i < region.len() {
                let p = region[i];
                if distance(center, [p.x as f64, p.y as f64]) > radius {
                    self.used[[p.y, p.x]] = false;
                    region.swap_remove(i);
                } else {
                    i += 1;
                }
            }
            if region.len() < 2 {
                return false;
            }
            *rect = self.rectangle(region, angle, prec);
        }
        true
    }
}

/// Port of cv2.createLineSegmentDetector(LSD_REFINE_STD).detect(gray)[0] in luma/upright.py.
/// Uses all OpenCV defaults (scale .8, sigma scale .6, quantization 2, angle 22.5, density .7, 1024 bins).
/// Returns ordered (x1,y1,x2,y2) float32 segments; no detections are an empty vector.
pub fn line_segment_detector(src: &Array2<u8>) -> Result<Vec<[f32; 4]>> {
    array::view(src)?;
    let (_, image) = preprocess(src)?;
    let (h, w) = image.dim();
    if h < 2 || w < 2 {
        return Ok(Vec::new());
    }
    let prec = PI * 22.5 / 180.0;
    let rho = 2.0 / prec.sin();
    let mut detector = Detector {
        angles: Array2::from_elem((h, w), -1024.0),
        gradient: Array2::zeros((h, w)),
        used: Array2::from_elem((h, w), false),
    };
    let mut max = -1.0_f64;
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let da = image[[y + 1, x + 1]] as i32 - image[[y, x]] as i32;
            let bc = image[[y, x + 1]] as i32 - image[[y + 1, x]] as i32;
            let gx = da + bc;
            let gy = da - bc;
            let norm = ((gx * gx + gy * gy) as f64 / 4.0).sqrt();
            detector.gradient[[y, x]] = norm;
            if norm > rho {
                detector.angles[[y, x]] = fast_atan(gx as f32, -gy as f32) * (PI / 180.0);
                max = max.max(norm);
            }
        }
    }
    let coefficient = if max > 0.0 { 1023.0 / max } else { 0.0 };
    let mut ordered = Vec::new();
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            ordered.push(((detector.gradient[[y, x]] * coefficient) as i32, y, x));
        }
    }
    ordered.sort_by_key(|a| std::cmp::Reverse(a.0));
    let log_nt = 5.0 * ((w as f64).log10() + (h as f64).log10()) / 2.0 + 11.0_f64.log10();
    let minimum = (-log_nt / 0.125_f64.log10()) as usize;
    let mut out = Vec::new();
    for (_, y, x) in ordered {
        if detector.used[[y, x]] || detector.angles[[y, x]] == -1024.0 {
            continue;
        }
        let (mut region, angle) = detector.grow(x, y, prec);
        if region.len() < minimum {
            continue;
        }
        let mut rect = detector.rectangle(&region, angle, prec);
        if !detector.refine(&mut region, prec, &mut rect) {
            continue;
        }
        out.push([
            ((rect.a[0] + 0.5) / 0.8) as f32,
            ((rect.a[1] + 0.5) / 0.8) as f32,
            ((rect.b[0] + 0.5) / 0.8) as f32,
            ((rect.b[1] + 0.5) / 0.8) as f32,
        ]);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lsd_byte_preprocessing() {
        let case = grainy_testkit::Case::open("imgproc", "tier2_lines");
        for (i, _) in case.json()["variants"].as_array().unwrap().iter().enumerate() {
            let (blurred, scaled) = preprocess(&case.array(&format!("input_{i}"))).unwrap();
            assert_eq!(blurred, case.array::<u8, _>(&format!("blurred_{i}")), "LSD blur {i}");
            assert_eq!(scaled, case.array::<u8, _>(&format!("scaled_{i}")), "LSD resize {i}");
        }
    }
}
