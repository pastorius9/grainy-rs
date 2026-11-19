//! Select Sky: a deterministic sky selection from colour, smoothness and position (no learned model).
//! Port of `luma/sky_mask.py`.
//!
//! The sky is found on a reduced copy of the developed colour source in the displayed frame's orientation:
//! bright blue, white/grey or warm smooth pixels in the upper part, kept only where they connect to the top of
//! the frame without crossing a sharp edge, with enclosed clouds filled. A colour guided filter (He et al.) then
//! turns that selection into soft edges that follow the photo, evaluated at the source's own resolution.
//!
//! Arithmetic follows the oracle's dtypes: float32 arrays, float64 Python scalars (thresholds are computed in
//! double and rounded to float where numpy does), numpy's pairwise sums and linear percentiles (`np` module) and
//! bit-exact ports of the OpenCV filters it calls (`cv` module). The one known difference is `cv2.magnitude`,
//! which the oracle's OpenCV build hands to Intel IPP (see `cv::deriv`).

mod cv;
mod np;

use grainy_core::{Image, Plane};
use grainy_imgproc::{
    connected_components, connected_components_with_stats, dilate, flood_fill, get_structuring_element, hough_lines_p,
    remap, Border, Interpolation, MorphShape,
};
use ndarray::{s, Array2, Array3};
use serde_json::Value;

use cv::Mat;

/// Long side of the detection copy (`DETECT_SIZE`); sizes below scale with it.
pub const DETECT_SIZE: usize = 640;
/// Rec. 709 luminance weights (`LUMA`, float32).
pub const LUMA: [f32; 3] = [0.2126, 0.7152, 0.0722];
/// `DEFAULTS['sensitivity']`: reach into haze and cloud, 0..100.
pub const DEFAULT_SENSITIVITY: f64 = 50.0;
/// `DEFAULTS['feather']`: edge softness, 0..100.
pub const DEFAULT_FEATHER: f64 = 30.0;

/// Errors of the sky mask.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An OpenCV-equivalent primitive rejected its input.
    #[error(transparent)]
    Imgproc(#[from] grainy_imgproc::Error),
    /// The input image or an argument has an unsupported shape or value.
    #[error("invalid input: {0}")]
    Input(String),
    /// The geometry transform failed.
    #[error("transform failed: {0}")]
    Transform(String),
}

/// Result type of this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// The displayed frame's geometry as a closure on H×W×C arrays (the oracle's `lambda m: geometry(m, s, crop)`).
pub type Transform<'a> = &'a dyn Fn(&Image) -> Result<Image>;

// ------------------------------------------------------------------------------------------------ helpers

/// numpy's `maximum`/`minimum` (a NaN operand wins).
fn np_max(a: f32, b: f32) -> f32 {
    if a.is_nan() || a >= b {
        a
    } else {
        b
    }
}

fn np_min(a: f32, b: f32) -> f32 {
    if a.is_nan() || a <= b {
        a
    } else {
        b
    }
}

/// Python's `round()` of a float to an integer (ties to even).
fn round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

fn mat(a: &Image) -> Mat {
    let (h, w, cn) = a.dim();
    Mat { h, w, cn, data: a.iter().copied().collect() }
}

fn mat1(a: &Plane) -> Mat {
    let (h, w) = a.dim();
    Mat { h, w, cn: 1, data: a.iter().copied().collect() }
}

fn image(m: Mat) -> Result<Image> {
    Array3::from_shape_vec((m.h, m.w, m.cn), m.data).map_err(|e| Error::Input(e.to_string()))
}

fn plane(m: Mat) -> Result<Plane> {
    Array2::from_shape_vec((m.h, m.w * m.cn), m.data).map_err(|e| Error::Input(e.to_string()))
}

fn flat<T: Clone>(a: &Array2<T>) -> Vec<T> {
    a.iter().cloned().collect()
}

/// `np.ascontiguousarray(np.rot90(a, k))` (counter-clockwise quarter turns).
fn rot90<T: Clone>(a: &Array3<T>, k: i64) -> Array3<T> {
    let k = k.rem_euclid(4);
    let (h, w, c) = a.dim();
    let (oh, ow) = if k % 2 == 1 { (w, h) } else { (h, w) };
    Array3::from_shape_fn((oh, ow, c), |(i, j, ch)| match k {
        0 => a[[i, j, ch]].clone(),
        1 => a[[j, w - 1 - i, ch]].clone(),
        2 => a[[h - 1 - i, w - 1 - j, ch]].clone(),
        _ => a[[h - 1 - j, i, ch]].clone(),
    })
}

/// Port of luma/sky_mask.py `_box`: `cv2.boxFilter(a, -1, (2r+1, 2r+1), borderType=BORDER_REFLECT_101)`.
pub fn box_filter(a: &Image, r: usize) -> Result<Image> {
    image(cv::box_filter(&mat(a), 2 * r + 1))
}

fn box_plane(a: &[f32], h: usize, w: usize, r: usize) -> Vec<f32> {
    cv::box_filter(&Mat { h, w, cn: 1, data: a.to_vec() }, 2 * r + 1).data
}

/// The scalars of `_ramp(x, low, high)`: `np.float32(low)` and `np.float32(1/(high-low))`.
fn ramp_args(low: f64, high: f64) -> (f32, f32) {
    (low as f32, (1.0 / (high - low)) as f32)
}

fn ramp1(x: f32, (low, k): (f32, f32)) -> f32 {
    np_min(np_max((x - low) * k, 0.0), 1.0)
}

/// Port of luma/sky_mask.py `_ramp`: `clip((x - low)/(high - low), 0, 1)` in float32.
pub fn ramp(x: &Plane, low: f64, high: f64) -> Plane {
    let args = ramp_args(low, high);
    x.mapv(|v| ramp1(v, args))
}

// ------------------------------------------------------------------------------------------------ view

/// Source pixels `(x0, y0, x1, y1)` of the shown frame and the quarter turns that bring its top up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View {
    /// `(x0, y0, x1, y1)` in source pixels.
    pub rect: [usize; 4],
    /// Counter-clockwise quarter turns (`np.rot90`) that bring the frame's top up.
    pub turns: usize,
}

/// Port of luma/sky_mask.py `view`: probes the transform with a coordinate image.
pub fn view(shape: (usize, usize), transform: Option<Transform>) -> Result<View> {
    let (h, w) = shape;
    let whole = View { rect: [0, 0, w, h], turns: 0 };
    let Some(transform) = transform else { return Ok(whole) };
    let scale = 96.0 / h.max(w) as f64;
    let ph = round(h as f64 * scale).max(8) as usize;
    let pw = round(w as f64 * scale).max(8) as usize;
    let probe = Array3::from_shape_fn((ph, pw, 3), |(y, x, c)| match c {
        0 => (x as f32 + 0.5) / pw as f32,
        1 => (y as f32 + 0.5) / ph as f32,
        _ => 1.0,
    });
    let probe = transform(&probe)?;
    if probe.ndim() != 3 || probe.shape()[2] < 3 {
        return Err(Error::Transform("the transform must keep three channels".into()));
    }
    let (qh, qw, _) = probe.dim();
    let (mut rows, mut sx, mut sy) = (Vec::new(), Vec::new(), Vec::new());
    for y in 0..qh {
        for x in 0..qw {
            if probe[[y, x, 2]] > 0.9f32 {
                rows.push(y as i64);
                sx.push(probe[[y, x, 0]]);
                sy.push(probe[[y, x, 1]]);
            }
        }
    }
    if rows.len() < 4 {
        return Ok(whole);
    }
    let p20 = np::percentile_i64(&mut rows.clone(), 20.0);
    let p80 = np::percentile_i64(&mut rows.clone(), 80.0);
    let pick = |v: &[f32], keep: &dyn Fn(i64) -> bool| -> Vec<f32> {
        rows.iter().zip(v).filter(|(r, _)| keep(**r)).map(|(_, &x)| x).collect()
    };
    let top = |r: i64| r as f64 <= p20;
    let bottom = |r: i64| r as f64 >= p80;
    let dx = (np::mean_f32(&pick(&sx, &top)) - np::mean_f32(&pick(&sx, &bottom))) * w as f32;
    let dy = (np::mean_f32(&pick(&sy, &top)) - np::mean_f32(&pick(&sy, &bottom))) * h as f32;
    let turns = if dx.abs() <= dy.abs() {
        if dy <= 0.0 {
            0
        } else {
            2
        }
    } else if dx > 0.0 {
        1
    } else {
        3
    };
    let margin = (1.5f64 / 96.0) as f32;
    let fold = |v: &[f32], f: fn(f32, f32) -> f32| v.iter().copied().reduce(f).unwrap_or(0.0);
    let clip = |v: f32| np_min(np_max(v, 0.0), 1.0);
    let x0 = clip(fold(&sx, np_min) - margin);
    let x1 = clip(fold(&sx, np_max) + margin);
    let y0 = clip(fold(&sy, np_min) - margin);
    let y1 = clip(fold(&sy, np_max) + margin);
    let left = (x0 * w as f32) as usize;
    let upper = (y0 * h as f32) as usize;
    let right = (left + 1).max((x1 * w as f32).ceil() as usize);
    let lower = (upper + 1).max((y1 * h as f32).ceil() as usize);
    Ok(View { rect: [left, upper, right, lower], turns })
}

// ------------------------------------------------------------------------------------------------ features

/// The per-pixel cues of `features`.
#[derive(Clone, Debug)]
pub struct Features {
    /// Sky likelihood 0..1.
    pub score: Plane,
    /// The blue part of it.
    pub blue: Plane,
    /// Normalised luminance.
    pub l: Plane,
    /// Local texture.
    pub texture: Plane,
    /// Sharp edges (in any channel).
    pub edges: Array2<bool>,
}

/// `cv2.magnitude` of the x and y Sobel derivatives of `a` (`features.magnitude`).
fn gradient_magnitude(a: &Mat, scale: f64) -> Vec<f32> {
    let gx = cv::sobel(a, true, scale);
    let gy = cv::sobel(a, false, scale);
    cv::magnitude(&gx.data, &gy.data)
}

/// Port of luma/sky_mask.py `features` for a detection copy whose first row is the top of the frame.
pub fn features(rgb: &Image, sensitivity: f64) -> Result<Features> {
    let (h, w, c) = rgb.dim();
    if h == 0 || w == 0 || c != 3 {
        return Err(Error::Input("features needs a non-empty RGB image".into()));
    }
    let s = sensitivity.clamp(0.0, 100.0) / 100.0;
    let s = if sensitivity.is_nan() { f64::NAN } else { s };
    let unit = h.max(w) as f64 / DETECT_SIZE as f64;
    // Exposure-independent: scaled to the photo's white by its brightest channel.
    let mut peaks: Vec<f32> = Vec::with_capacity(h.div_ceil(2) * w.div_ceil(2));
    for y in (0..h).step_by(2) {
        for x in (0..w).step_by(2) {
            peaks.push(np_max(np_max(rgb[[y, x, 0]], rgb[[y, x, 1]]), rgb[[y, x, 2]]));
        }
    }
    let white = f64::from(np::percentile_f32(&mut peaks, 99.5)).max(0.05);
    let gain = (1.0 / white) as f32;
    let mut n = Mat::zeros(h, w, 3);
    for (i, v) in n.data.iter_mut().enumerate() {
        *v = np_max(np_min(rgb[[i / (3 * w), i / 3 % w, i % 3]] * gain, 1.0), 0.0);
    }
    let hsv = cv::rgb_to_hsv(&n);
    let l: Vec<f32> =
        n.data.as_chunks::<3>().0.iter().map(|p| (p[0] * LUMA[0] + p[1] * LUMA[1]) + p[2] * LUMA[2]).collect();
    let sigma = (0.8 * unit).max(0.5);
    let scale = unit.max(0.25) / 8.0;
    let blurred = cv::gaussian_blur(&Mat { h, w, cn: 1, data: l.clone() }, sigma);
    let gradient = gradient_magnitude(&blurred, scale);
    let texture = box_plane(&gradient, h, w, round(2.0 * unit).max(1) as usize);
    let colour: Vec<f32> = gradient_magnitude(&cv::gaussian_blur(&n, sigma), scale)
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| p[1..].iter().fold(p[0], |m, &v| np_max(m, v)))
        .collect();
    let r_blue = [ramp_args(175.0, 195.0), ramp_args(250.0, 275.0), ramp_args(0.04, 0.12)];
    let r_bl = ramp_args(0.30 - 0.12 * s, 0.45 - 0.12 * s);
    let r_warm = [ramp_args(45.0, 65.0), ramp_args(315.0, 335.0)];
    let r_pale = [ramp_args(0.08 + 0.08 * s, 0.20 + 0.08 * s), ramp_args(0.55 - 0.2 * s, 0.75 - 0.2 * s)];
    let r_sunset = [ramp_args(0.55, 0.85), ramp_args(0.6 - 0.15 * s, 0.8 - 0.15 * s)];
    let r_calm = ramp_args(0.012 + 0.02 * s, 0.03 + 0.04 * s);
    let r_prior = ramp_args(0.3, 1.0);
    let edge = (0.035 + 0.03 * s) as f32;
    let step = if h > 1 { 1.0 / (h - 1) as f64 } else { f64::NAN };
    let prior: Vec<f32> = (0..h)
        .map(|y| {
            let t = if h > 1 && y == h - 1 { 1.0 } else { (y as f64 * step) as f32 };
            let t = if h == 1 { 0.0 } else { t };
            1.0 - 0.55f32 * ramp1(t, r_prior)
        })
        .collect();
    let (mut score, mut blue_out) = (vec![0f32; h * w], vec![0f32; h * w]);
    for i in 0..h * w {
        let (hue, sat, lum) = (hsv.data[3 * i], hsv.data[3 * i + 1], l[i]);
        let blue = ramp1(hue, r_blue[0]) * (1.0 - ramp1(hue, r_blue[1])) * ramp1(sat, r_blue[2]) * ramp1(lum, r_bl);
        let warm = np_max(1.0 - ramp1(hue, r_warm[0]), ramp1(hue, r_warm[1]));
        let pale = np_max(
            (1.0 - ramp1(sat, r_pale[0])) * ramp1(lum, r_pale[1]),
            0.85f32 * warm * (1.0 - ramp1(sat, r_sunset[0])) * ramp1(lum, r_sunset[1]),
        );
        let calm = 1.0 - ramp1(texture[i], r_calm);
        let p = prior[i / w];
        score[i] = np_max(blue, pale) * calm * p;
        blue_out[i] = blue * calm * p;
    }
    let shape = |v: Vec<f32>| Array2::from_shape_vec((h, w), v).map_err(|e| Error::Input(e.to_string()));
    Ok(Features {
        score: shape(score)?,
        blue: shape(blue_out)?,
        l: shape(l)?,
        texture: shape(texture)?,
        edges: Array2::from_shape_vec((h, w), colour.iter().map(|&v| v > edge).collect())
            .map_err(|e| Error::Input(e.to_string()))?,
    })
}

// ------------------------------------------------------------------------------------------------ horizon

/// Port of luma/sky_mask.py `horizon`: the row (per column, float64) of a straight, level edge that darkens
/// downwards with sky-like pixels on both sides, such as a sea horizon; `None` when there is none.
pub fn horizon(l: &Plane, candidates: &Array2<u8>) -> Result<Option<Vec<f64>>> {
    let (h, w) = l.dim();
    if candidates.dim() != (h, w) || h == 0 || w == 0 {
        return Err(Error::Input("horizon needs a luminance plane and candidates of one shape".into()));
    }
    let unit = h.max(w) as f64 / DETECT_SIZE as f64;
    let k = round(3.0 * unit).max(2) as usize;
    let smooth = cv::gaussian_blur(&mat1(l), unit.max(0.5)).data;
    let cand = flat(candidates);
    // drop[k:h-k] = smooth[:h-2k] - smooth[2k:]; dilate with a (2k+1)×1 column of ones.
    let mut edge = Array2::<u8>::zeros((h, w));
    let threshold = 0.06f32;
    if h > 2 * k {
        for y in k..h - k {
            for x in 0..w {
                let drop = smooth[(y - k) * w + x] - smooth[(y + k) * w + x];
                let near = (y.saturating_sub(k)..=(y + k).min(h - 1)).any(|yy| cand[yy * w + x] > 0);
                if drop > threshold && near {
                    edge[[y, x]] = 255;
                }
            }
        }
    }
    let lines = hough_lines_p(
        &edge,
        1.0,
        std::f64::consts::PI / 720.0,
        round(0.12 * w as f64).max(20) as i32,
        0.25 * w as f64,
        round(0.01 * w as f64).max(2) as f64,
    )?;
    let mut best: Option<[i32; 4]> = None;
    let (hi, ki) = (h as i64, k as i64);
    for &[x0, y0, x1, y1] in &lines {
        if x1 == x0 || f64::from((y1 - y0).abs()) > 0.1 * f64::from((x1 - x0).abs()) {
            continue;
        }
        let xs: Vec<i64> = (i64::from(x0.min(x1))..=i64::from(x0.max(x1))).collect();
        let ys: Vec<i64> = xs
            .iter()
            .map(|&x| {
                let num = (x - i64::from(x0)) * i64::from(y1 - y0);
                (f64::from(y0) + num as f64 / f64::from(x1 - x0)).round_ties_even() as i64
            })
            .collect();
        let at = |y: i64, x: i64| (y.clamp(0, hi - 1) * w as i64 + x) as usize;
        let n = xs.len() as f64;
        let above = xs.iter().zip(&ys).filter(|(&x, &y)| cand[at(y - 2 * ki, x)] > 0).count() as f64;
        let below_count = xs.iter().zip(&ys).filter(|(&x, &y)| cand[at(y + 2 * ki, x)] > 0).count() as f64;
        // Sky above all along, and below it enough sky-like water or haze for the sky to leak into.
        if above / n < 0.85 || below_count / n < 0.3 {
            continue;
        }
        // Water and land below a horizon are darker than the sky above it; a cloud base is not.
        let top = (y0.min(y1).max(0) as usize).max(1).min(h);
        let mut sky: Vec<f32> = (0..top * w).filter(|&i| cand[i] > 0).map(|i| smooth[i]).collect();
        if sky.is_empty() {
            continue;
        }
        let under: Vec<f32> = xs.iter().zip(&ys).map(|(&x, &y)| smooth[at(y + 2 * ki, x)]).collect();
        if np::mean_f32(&under) > np::median_f32(&mut sky) - 0.06f32 {
            continue;
        }
        if best.is_none_or(|b| y0.min(y1) < b[1]) {
            best = Some([x0, y0, x1, y1]);
        }
    }
    Ok(best.map(|[x0, y0, x1, y1]| {
        (0..w as i64)
            .map(|x| f64::from(y0) + ((x - i64::from(x0)) * i64::from(y1 - y0)) as f64 / f64::from(x1 - x0))
            .collect()
    }))
}

// ------------------------------------------------------------------------------------------------ detect

fn candidates_of(f: &Features) -> Array2<u8> {
    ndarray::Zip::from(&f.score).and(&f.edges).map_collect(|&s, &e| u8::from(s > 0.5f32 && !e))
}

/// `np.arange(h)[:, None] >= line[None, :] - 1`.
fn below_line(h: usize, line: &[f64]) -> Array2<bool> {
    Array2::from_shape_fn((h, line.len()), |(y, x)| y as f64 >= line[x] - 1.0)
}

/// Port of luma/sky_mask.py `detect`: the binary sky of a detection copy (first row = top of the frame); empty
/// when no sky reaches the top. Regions are chosen at the default sensitivity; other sensitivities let that sky
/// reach further into haze and cloud, or keep its clearest part.
pub fn detect(rgb: &Image, sensitivity: f64) -> Result<Array2<bool>> {
    let (h, w, _) = rgb.dim();
    let empty = Array2::from_elem((h, w), false);
    let f = features(rgb, DEFAULT_SENSITIVITY)?;
    let mut candidates = candidates_of(&f);
    let line = horizon(&f.l, &candidates)?;
    let below = line.as_deref().map(|line| below_line(h, line));
    if let Some(below) = &below {
        ndarray::Zip::from(&mut candidates).and(below).for_each(|c, &b| {
            if b {
                *c = 0;
            }
        });
    }
    let cc = connected_components_with_stats(&candidates, 4)?;
    let count = cc.count;
    if count < 2 {
        return Ok(empty);
    }
    let labels: Vec<usize> = cc.labels.iter().map(|&v| v as usize).collect();
    let stat = |i: usize, k: usize| cc.stats[[i, k]];
    let area: Vec<f64> = (0..count).map(|i| f64::from(stat(i, 4))).collect();
    let mut blue_sum = vec![0f64; count];
    let mut bright_sum = vec![0f64; count];
    for ((&label, &b), &l) in labels.iter().zip(f.blue.iter()).zip(f.l.iter()) {
        blue_sum[label] += if b > 0.5f32 { 1.0 } else { 0.0 };
        bright_sum[label] += f64::from(l);
    }
    let blue: Vec<f64> = (0..count).map(|i| blue_sum[i] / area[i].max(1.0)).collect();
    let bright: Vec<f64> = (0..count).map(|i| bright_sum[i] / area[i].max(1.0)).collect();
    let band = round(0.03 * h as f64).max(2) as usize;
    // Top-band columns per component.
    let mut width = vec![0usize; count];
    {
        let mut seen = std::collections::HashSet::new();
        for y in 0..band.min(h) {
            for x in 0..w {
                let label = labels[y * w + x];
                if seen.insert((label, x)) {
                    width[label] += 1;
                }
            }
        }
    }
    let (hf, wf) = (h as f64, w as f64);
    let seeded: Vec<bool> = (0..count).map(|i| i > 0 && width[i] > 0).collect();
    let high: Vec<bool> = (0..count).map(|i| f64::from(stat(i, 1)) <= 0.15 * hf).collect();
    let mut accept: Vec<bool> = (0..count)
        .map(|i| {
            blue[i] >= 0.5 && ((seeded[i] && area[i] >= 0.002 * hf * wf) || (high[i] && area[i] >= 0.01 * hf * wf))
        })
        .collect();
    // A pale region that runs most of the way down is a wall, a floor or a light leak rather than sky.
    let pale: Vec<bool> = (0..count)
        .map(|i| {
            seeded[i]
                && !accept[i]
                && (width[i] as f64 >= 0.2 * wf || area[i] >= 0.02 * hf * wf)
                && bright[i] >= 0.55
                && f64::from(stat(i, 1) + stat(i, 3)) <= 0.75 * hf
        })
        .collect();
    for label in (0..count).filter(|&i| pale[i]) {
        // Walls between windows are pale and smooth too; a pale sky has few enclosed gaps.
        let (x, y, bw, bh) =
            (stat(label, 0) as usize, stat(label, 1) as usize, stat(label, 2) as usize, stat(label, 3) as usize);
        let mut outside = Array2::<u8>::from_elem((bh + 2, bw + 2), 1);
        for yy in 0..bh {
            for xx in 0..bw {
                outside[[yy + 1, xx + 1]] = u8::from(labels[(y + yy) * w + x + xx] != label);
            }
        }
        let filled = flood_fill(&outside, (0, 0), 2)?.image;
        let holes = filled.slice(s![1..bh + 1, 1..bw + 1]).iter().filter(|&&v| v == 1).count();
        accept[label] = holes as f64 <= 0.12 * area[label];
    }
    accept[0] = false;
    if !accept.iter().any(|&a| a) {
        return Ok(empty);
    }
    // Sky seen through foliage: blue, smooth pieces above the sky's lower reach in a similar brightness.
    let mut rows: Vec<i64> =
        labels.iter().enumerate().filter(|(_, &l)| accept[l]).map(|(i, _)| (i / w) as i64).collect();
    let reach = np::percentile_i64(&mut rows, 90.0);
    let accepted: Vec<usize> = (0..count).filter(|&i| accept[i]).collect();
    let weighted: Vec<f64> = accepted.iter().map(|&i| bright[i] * area[i]).collect();
    let weights: Vec<f64> = accepted.iter().map(|&i| area[i]).collect();
    let average = np::pairwise(&weighted) / np::pairwise(&weights);
    let loose: Vec<bool> = (0..count)
        .map(|i| {
            i > 0
                && !accept[i]
                && blue[i] >= 0.6
                && cc.centroids[[i, 1]] <= reach
                && area[i] >= 0.0002 * hf * wf
                && (bright[i] - average).abs() < 0.15
        })
        .collect();
    let mut sky: Vec<bool> = labels.iter().map(|&l| accept[l] || loose[l]).collect();
    let mut texture = f.texture;
    if sensitivity != DEFAULT_SENSITIVITY {
        let f = features(rgb, sensitivity)?;
        let mut reach = candidates_of(&f);
        if let Some(below) = &below {
            ndarray::Zip::from(&mut reach).and(below).for_each(|c, &b| {
                if b {
                    *c = 0;
                }
            });
        }
        // Water that only now looks like sky.
        if let Some(line) = horizon(&f.l, &reach)? {
            let below = below_line(h, &line);
            ndarray::Zip::from(&mut reach).and(&below).for_each(|c, &b| {
                if b {
                    *c = 0;
                }
            });
        }
        let (count, labels) = connected_components(&reach, 4)?;
        let mut keep = vec![false; count];
        for ((&label, &r), &s) in labels.iter().zip(reach.iter()).zip(&sky) {
            if s && r > 0 {
                keep[label as usize] = true;
            }
        }
        keep[0] = false;
        sky = labels.iter().map(|&l| keep[l as usize]).collect();
        texture = f.texture;
    }
    // Enclosed clouds, birds and wires: holes that reach no side or bottom edge.
    let not_sky = Array2::from_shape_vec((h, w), sky.iter().map(|&s| u8::from(!s)).collect())
        .map_err(|e| Error::Input(e.to_string()))?;
    let holes = connected_components_with_stats(&not_sky, 4)?;
    let count = holes.count;
    let mut edge = vec![false; count];
    for y in 0..h {
        edge[holes.labels[[y, 0]] as usize] = true;
        edge[holes.labels[[y, w - 1]] as usize] = true;
    }
    for x in 0..w {
        edge[holes.labels[[h - 1, x]] as usize] = true;
    }
    let total = sky.iter().filter(|&&s| s).count() as f64;
    let mut texture_sum = vec![0f64; count];
    for (&label, &t) in holes.labels.iter().zip(texture.iter()) {
        texture_sum[label as usize] += f64::from(t);
    }
    let fill: Vec<bool> = (0..count)
        .map(|i| {
            let a = holes.stats[[i, 4]];
            let minor = f64::from(a) < 0.25 * total;
            let calm = texture_sum[i] / f64::from(a).max(1.0) < 0.03;
            let tiny = f64::from(a) < 0.002 * hf * wf;
            i > 0 && !edge[i] && minor && (calm || tiny)
        })
        .collect();
    let out: Vec<bool> = sky.iter().zip(holes.labels.iter()).map(|(&s, &l)| s || fill[l as usize]).collect();
    Array2::from_shape_vec((h, w), out).map_err(|e| Error::Input(e.to_string()))
}

// ------------------------------------------------------------------------------------------------ guided filter

/// Port of luma/sky_mask.py `_guided_coefficients`: box-averaged `(a_r, a_g, a_b, b)` of a colour guided filter
/// of `p` (H×W) guided by `guide` (H×W×3), so `q = a·I + b` at any resolution.
pub fn guided_coefficients(guide: &Image, p: &Plane, r: usize, eps: f32) -> Result<Image> {
    let (h, w, c) = guide.dim();
    if c != 3 || p.dim() != (h, w) {
        return Err(Error::Input("the guide must be H×W×3 and p H×W".into()));
    }
    let n = h * w;
    let g = mat(guide);
    let pv: Vec<f32> = p.iter().copied().collect();
    let bx3 = |data: Vec<f32>| cv::box_filter(&Mat { h, w, cn: 3, data }, 2 * r + 1).data;
    let bx1 = |data: &[f32]| box_plane(data, h, w, r);
    let m = bx3(g.data.clone());
    let mp = bx1(&pv);
    let gp: Vec<f32> = (0..3 * n).map(|i| g.data[i] * pv[i / 3]).collect();
    let bgp = bx3(gp);
    let cov: Vec<f32> = (0..3 * n).map(|i| bgp[i] - m[i] * mp[i / 3]).collect();
    let product = |a: usize, b: usize| -> Vec<f32> {
        bx1(&(0..n).map(|i| g.data[3 * i + a] * g.data[3 * i + b]).collect::<Vec<_>>())
    };
    let (p00, p01, p02, p11, p12, p22) =
        (product(0, 0), product(0, 1), product(0, 2), product(1, 1), product(1, 2), product(2, 2));
    let mut a = vec![0f32; 3 * n];
    let mut b = vec![0f32; n];
    for i in 0..n {
        let (m0, m1, m2) = (m[3 * i], m[3 * i + 1], m[3 * i + 2]);
        let rr = p00[i] - m0 * m0 + eps;
        let rg = p01[i] - m0 * m1;
        let rb = p02[i] - m0 * m2;
        let gg = p11[i] - m1 * m1 + eps;
        let gb = p12[i] - m1 * m2;
        let bb = p22[i] - m2 * m2 + eps;
        let i11 = gg * bb - gb * gb;
        let i12 = rb * gb - rg * bb;
        let i13 = rg * gb - rb * gg;
        let i22 = rr * bb - rb * rb;
        let i23 = rb * rg - rr * gb;
        let i33 = rr * gg - rg * rg;
        let det = rr * i11 + rg * i12 + rb * i13;
        let (c0, c1, c2) = (cov[3 * i], cov[3 * i + 1], cov[3 * i + 2]);
        let a0 = (i11 * c0 + i12 * c1 + i13 * c2) / det;
        let a1 = (i12 * c0 + i22 * c1 + i23 * c2) / det;
        let a2 = (i13 * c0 + i23 * c1 + i33 * c2) / det;
        a[3 * i] = a0;
        a[3 * i + 1] = a1;
        a[3 * i + 2] = a2;
        b[i] = mp[i] - ((a0 * m0 + a1 * m1) + a2 * m2);
    }
    let ba = bx3(a);
    let bb = bx1(&b);
    Ok(Array3::from_shape_fn((h, w, 4), |(y, x, ch)| {
        let i = y * w + x;
        if ch < 3 {
            ba[3 * i + ch]
        } else {
            bb[i]
        }
    }))
}

/// Port of luma/sky_mask.py `_apply`: `q = a·I + b` with the coefficients (H'×W'×5: a, b and the reach)
/// bilinearly upsampled to the guide's size, clipped to `0..reach`.
pub fn apply(coefficients: &Image, guide: &Image) -> Result<Plane> {
    let (h, w, gc) = guide.dim();
    let (ch, cw, cc) = coefficients.dim();
    if cc != 5 || gc < 3 {
        return Err(Error::Input("coefficients must have five channels and the guide three".into()));
    }
    let u: Vec<f32> = (0..w).map(|x| (x as f32 + 0.5) * (cw as f64 / w as f64) as f32 - 0.5).collect();
    let fy = (ch as f64 / h as f64) as f32;
    let mx = Array2::from_shape_fn((h, w), |(_, x)| u[x]);
    let my = Array2::from_shape_fn((h, w), |(y, _)| (y as f32 + 0.5) * fy - 0.5);
    let c = remap(coefficients, &mx, &my, Interpolation::Linear, Border::Replicate)?;
    Ok(Array2::from_shape_fn((h, w), |(y, x)| {
        let q = c[[y, x, 0]] * guide[[y, x, 0]]
            + c[[y, x, 1]] * guide[[y, x, 1]]
            + c[[y, x, 2]] * guide[[y, x, 2]]
            + c[[y, x, 3]];
        np_min(np_max(q, 0.0), c[[y, x, 4]])
    }))
}

// ------------------------------------------------------------------------------------------------ sky_mask

/// What `detection_copy` returns: the shown frame's region of the source, its reduced copy turned top-up, and
/// the view.
#[derive(Clone, Debug)]
pub struct DetectionCopy {
    /// `source[y0:y1, x0:x1, :3]`.
    pub region: Image,
    /// The region reduced to at most `DETECT_SIZE` on its long side (INTER_AREA) and turned top-up.
    pub small: Image,
    /// The view (rectangle and turns).
    pub view: View,
}

/// Port of luma/sky_mask.py `detection_copy`.
pub fn detection_copy(source: &Image, transform: Option<Transform>) -> Result<DetectionCopy> {
    let (h, w, c) = source.dim();
    if h == 0 || w == 0 || c < 3 {
        return Err(Error::Input("the source must be a non-empty H×W×3 image".into()));
    }
    let view = view((h, w), transform)?;
    let [x0, y0, x1, y1] = view.rect;
    let (x1, y1) = (x1.min(w), y1.min(h));
    if x0 >= x1 || y0 >= y1 {
        return Err(Error::Input("the shown frame is empty".into()));
    }
    let region = source.slice(s![y0..y1, x0..x1, 0..3]).to_owned();
    let (rh, rw) = (y1 - y0, x1 - x0);
    let factor = (DETECT_SIZE as f64 / rh.max(rw) as f64).min(1.0);
    let small = if factor < 1.0 {
        let dw = round(rw as f64 * factor).max(1) as usize;
        let dh = round(rh as f64 * factor).max(1) as usize;
        image(cv::resize_area(&mat(&region), dw, dh))?
    } else {
        region.clone()
    };
    let small = rot90(&small, view.turns as i64);
    Ok(DetectionCopy { region, small, view })
}

fn number(component: &Value, key: &str, default: f64) -> Result<f64> {
    match component.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(Value::Bool(b)) => Ok(f64::from(u8::from(*b))),
        Some(Value::Number(n)) => n.as_f64().ok_or_else(|| Error::Input(format!("{key} is not a number"))),
        Some(_) => Err(Error::Input(format!("{key} is not a number"))),
    }
}

/// The middle of `sky_mask`: the half-size guide, the detection `p`, the filter radius and epsilon, the reach and
/// the guided-filter coefficients with the reach as a fifth channel (in the detection copy's orientation).
#[derive(Clone, Debug)]
pub struct Guided {
    /// `cv2.resize(small, size, INTER_AREA)`.
    pub guide: Image,
    /// `cv2.resize(sky as float32, size, INTER_AREA)`.
    pub p: Plane,
    /// Filter radius.
    pub r: usize,
    /// Regulariser (float32).
    pub eps: f32,
    /// `_box(dilate(p, ellipse), max(1, r//2))` (not clipped to 1).
    pub reach: Plane,
    /// `_guided_coefficients(guide, p, r, eps)`.
    pub coefficients: Image,
}

/// The guided-filter stage of luma/sky_mask.py `sky_mask` for a detection copy and its detected sky.
pub fn guided(small: &Image, sky: &Array2<bool>, feather: f64) -> Result<Guided> {
    let feather = feather.clamp(0.0, 100.0) / 100.0;
    let (sh, sw, _) = small.dim();
    let (dw, dh) = ((sw / 2).max(1), (sh / 2).max(1));
    let guide = image(cv::resize_area(&mat(small), dw, dh))?;
    let sky_f = Mat { h: sh, w: sw, cn: 1, data: sky.iter().map(|&s| if s { 1.0 } else { 0.0 }).collect() };
    let p = plane(cv::resize_area(&sky_f, dw, dh))?;
    let unit = dw.max(dh) as f64 / (DETECT_SIZE as f64 / 2.0);
    let r = round((1.0 + 5.0 * feather) * unit).max(1) as usize;
    let eps = (1e-4 + 3e-3 * feather) as f32;
    // The linear model learns "brighter is sky" from dark branches and would carry it onto a white wall nearby;
    // the result stays within a soft margin of the detected sky.
    let grow = 1 + r / 2;
    let kernel = get_structuring_element(MorphShape::Ellipse, (2 * grow + 1, 2 * grow + 1), None)?;
    let dilated = dilate(&p, &kernel, None, 1, None)?;
    let reach = Array2::from_shape_vec((dh, dw), box_plane(&flat(&dilated), dh, dw, (r / 2).max(1)))
        .map_err(|e| Error::Input(e.to_string()))?;
    let coefficients = guided_coefficients(&guide, &p, r, eps)?;
    Ok(Guided { guide, p, r, eps, reach, coefficients })
}

/// Port of luma/sky_mask.py `sky_mask`: coverage 0..1 in source pixels for a `sky` component (its `sensitivity`
/// and `feather`, other keys ignored). `source` is the developed colour source (H×W×3 or more channels);
/// `transform` the displayed frame's geometry, used to find the frame's region and orientation.
pub fn sky_mask(source: &Image, component: &Value, transform: Option<Transform>) -> Result<Plane> {
    let (h, w, _) = source.dim();
    let mut result = Plane::zeros((h, w));
    let copy = detection_copy(source, transform)?;
    let (sh, sw, _) = copy.small.dim();
    if sh.min(sw) < 16 {
        return Ok(result);
    }
    let sky = detect(&copy.small, number(component, "sensitivity", DEFAULT_SENSITIVITY)?)?;
    if !sky.iter().any(|&s| s) {
        return Ok(result);
    }
    // The filter's coefficients are smooth over its window, so half the detection size carries them.
    let g = guided(&copy.small, &sky, number(component, "feather", DEFAULT_FEATHER)?)?;
    let (gh, gw, _) = g.coefficients.dim();
    let coefficients = Array3::from_shape_fn((gh, gw, 5), |(y, x, c)| {
        if c < 4 {
            g.coefficients[[y, x, c]]
        } else {
            np_min(g.reach[[y, x]], 1.0)
        }
    });
    let coefficients = rot90(&coefficients, -(copy.view.turns as i64));
    let q = apply(&coefficients, &copy.region)?;
    let [x0, y0, _, _] = copy.view.rect;
    let (rh, rw) = q.dim();
    result.slice_mut(s![y0..y0 + rh, x0..x0 + rw]).assign(&q);
    Ok(result)
}

#[cfg(test)]
mod tests;
