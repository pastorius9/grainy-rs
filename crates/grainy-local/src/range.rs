//! Manual range selections: `luma/range_mask.py` (`color_coverage`, `sample_selection`).

use std::ops::ControlFlow;

use grainy_core::{effective, Image, Plane, Settings};
use ndarray::{s, Axis};
use serde_json::{Map, Value};

use crate::np::{self, clip, dot3, maximum, minimum, norm3, py_int, py_max, sq3, LUMA};
use crate::value::number;
use crate::{Error, Result};

const BAD_COLOURS: &str = "색상 범위에는 유효한 색 1–5개가 필요합니다.";
const BAD_TOLERANCE: &str = "색상 범위 허용량이 올바르지 않습니다.";
const BAD_POINTS: &str = "사진 안쪽의 점이나 작은 사각형을 선택하세요.";
const NO_LAYER: &str = "선택한 마스크를 찾을 수 없습니다.";
const EMPTY_BORDER: &str = "변형 후 생긴 빈 가장자리 대신 사진 안쪽을 선택하세요.";
const NO_INPUT: &str = "선택한 마스크의 입력을 읽을 수 없습니다.";

fn invalid(message: &str) -> Error {
    Error::Invalid(message.to_owned())
}

/// Port of luma/range_mask.py `color_coverage`: the largest closeness (1 at a sample, 0 at `tolerance` RGB
/// distance) of each pixel to 1–5 sample colours.
pub fn color_coverage(image: &Image, samples: &Value, tolerance: f64) -> Result<Plane> {
    let list = samples.as_array().ok_or_else(|| invalid(BAD_COLOURS))?;
    let mut values = Vec::with_capacity(list.len());
    for rgb in list {
        let channels = rgb.as_array().ok_or_else(|| invalid(BAD_COLOURS))?;
        if channels.len() != 3 {
            return Err(invalid(BAD_COLOURS));
        }
        let mut v = [0f32; 3];
        for (slot, c) in v.iter_mut().zip(channels) {
            *slot = number(c).ok_or_else(|| invalid(BAD_COLOURS))? as f32;
        }
        values.push(v);
    }
    if !(1..=5).contains(&values.len()) || !values.iter().flatten().all(|v| v.is_finite()) {
        return Err(invalid(BAD_COLOURS));
    }
    if !tolerance.is_finite() || tolerance <= 0.0 {
        return Err(invalid(BAD_TOLERANCE));
    }
    let tol = py_max(0.01, tolerance) as f32;
    let mut result = Plane::zeros((image.dim().0, image.dim().1));
    for rgb in values {
        ndarray::Zip::from(&mut result).and(image.lanes(Axis(2))).for_each(|r, p| {
            let d = norm3([p[0] - rgb[0], p[1] - rgb[1], p[2] - rgb[2]]);
            *r = maximum(*r, clip(1.0 - d / tol, 0.0, 1.0));
        });
    }
    Ok(result)
}

/// What [`sample_selection`] returns: up to five representative colours and the luminance span of the selection.
#[derive(Clone, Debug, PartialEq)]
pub struct RangeSample {
    /// Representative colours (the first nearest the mean, then each farthest from those chosen).
    pub colors: Vec<[f32; 3]>,
    /// The `[low, high]` luminance of the valid selected pixels, clipped to 0..1.
    pub range: [f64; 2],
}

/// The selection logic of luma/range_mask.py `sample_selection` (its `read` callback) on the layer's input
/// `image`: the rectangle spanned by the first and last of `points` (normalised), restricted to the valid area
/// (`valid_frame`, the geometry of an all-ones source, above 0.9), its luminance span, and up to five colours
/// chosen on a 64×64 grid by farthest-point sampling.
pub fn select_colors(image: &Image, valid_frame: &Image, points: &[[f64; 2]]) -> Result<RangeSample> {
    let (h, w, _) = image.dim();
    let (Some(start), Some(end)) = (points.first(), points.last()) else { return Err(invalid(BAD_POINTS)) };
    let column = |p: &[f64; 2]| py_int(p[0] * w as f64).map(|v| v.min(w as i64 - 1));
    let row = |p: &[f64; 2]| py_int(p[1] * h as f64).map(|v| v.min(h as i64 - 1));
    let (Some(a), Some(b), Some(c), Some(d)) = (column(start), column(end), row(start), row(end)) else {
        return Err(invalid(BAD_POINTS));
    };
    let (x0, x1) = (a.min(b).max(0) as usize, a.max(b).max(0) as usize);
    let (y0, y1) = (c.min(d).max(0) as usize, c.max(d).max(0) as usize);
    // numpy slices stop at the end of the array.
    let slice = |n: usize, lo: usize, hi: usize| (lo.min(n), (hi + 1).min(n));
    let (vy, vx) = (slice(valid_frame.dim().0, y0, y1), slice(valid_frame.dim().1, x0, x1));
    let (py, px) = (slice(h, y0, y1), slice(w, x0, x1));
    let valid = valid_frame.slice(s![vy.0..vy.1, vx.0..vx.1, 0]).mapv(|v| v > 0.9_f64 as f32);
    let patch = image.slice(s![py.0..py.1, px.0..px.1, ..]);
    if valid.dim() != (patch.dim().0, patch.dim().1) {
        return Err(Error::Shape(format!(
            "the valid area {:?} does not fit the selection {:?}",
            valid.dim(),
            patch.dim()
        )));
    }
    if !valid.iter().any(|&v| v) {
        return Err(invalid(EMPTY_BORDER));
    }
    let luminance = patch.map_axis(Axis(2), |p| dot3(&[p[0], p[1], p[2]], &LUMA));
    let mut low = f32::INFINITY;
    let mut high = f32::NEG_INFINITY;
    let mut first = true;
    for (&l, &v) in luminance.iter().zip(&valid) {
        if v {
            // np.min/np.max: NaN propagates.
            if first {
                (low, high, first) = (l, l, false);
            } else {
                low = minimum(low, l);
                high = maximum(high, l);
            }
        }
    }
    let range = [f64::from(clip(low, 0.0, 1.0)), f64::from(clip(high, 0.0, 1.0))];
    // Bound colour selection work to a 64x64 grid, including rectangle edges.
    let (ph, pw) = (patch.dim().0, patch.dim().1);
    let ys = np::linspace_int(ph, ph.min(64));
    let xs = np::linspace_int(pw, pw.min(64));
    let mut candidates: Vec<[f32; 3]> = Vec::new();
    for &y in &ys {
        for &x in &xs {
            if valid[[y, x]] {
                candidates.push([patch[[y, x, 0]], patch[[y, x, 1]], patch[[y, x, 2]]]);
            }
        }
    }
    if candidates.is_empty() {
        if let Some(((y, x), _)) = valid.indexed_iter().find(|(_, &v)| v) {
            candidates.push([patch[[y, x, 0]], patch[[y, x, 1]], patch[[y, x, 2]]]);
        }
    }
    let mut sums = [0f32; 3];
    for c in &candidates {
        for k in 0..3 {
            sums[k] += c[k];
        }
    }
    let mean = sums.map(|v| np::mean_of_sum(v, candidates.len()));
    let distance = |a: &[f32; 3], b: &[f32; 3]| sq3([a[0] - b[0], a[1] - b[1], a[2] - b[2]]);
    let argmin = |values: &[f32]| first_index(values, |a, b| b < a);
    let argmax = |values: &[f32]| first_index(values, |a, b| b > a);
    let distances: Vec<f32> = candidates.iter().map(|c| distance(c, &mean)).collect();
    let mut chosen = vec![candidates[argmin(&distances)]];
    let mut nearest: Vec<f32> = candidates.iter().map(|c| distance(c, &chosen[0])).collect();
    loop {
        let largest = nearest.iter().copied().reduce(maximum).unwrap_or(f32::NAN);
        if chosen.len() >= 5 || f64::from(largest) <= 1e-6 || largest.is_nan() {
            break;
        }
        let rgb = candidates[argmax(&nearest)];
        chosen.push(rgb);
        for (n, c) in nearest.iter_mut().zip(&candidates) {
            *n = minimum(*n, distance(c, &rgb));
        }
    }
    Ok(RangeSample { colors: chosen, range })
}

/// `np.argmin`/`np.argmax`: the first index of the extreme value (the first NaN wins).
fn first_index(values: &[f32], better: impl Fn(f32, f32) -> bool) -> usize {
    if let Some(i) = values.iter().position(|v| v.is_nan()) {
        return i;
    }
    let mut best = 0;
    for (i, &v) in values.iter().enumerate().skip(1) {
        if better(values[best], v) {
            best = i;
        }
    }
    best
}

/// The develop the selection reads: called with the settings to render and the layer-input callback
/// (`develop(source, s, use_crop, output_space=None, original_size=..., on_layer_input=read)` in the oracle). It
/// must call the callback with each layer's input in order and stop when it returns `Break`.
pub type DevelopWithLayerInput<'a> =
    &'a mut dyn FnMut(&Settings, &mut dyn FnMut(usize, &Image) -> ControlFlow<()>) -> Result<()>;

/// The frame geometry (`engine.geometry(a, settings, use_crop)`) applied to an H×W×1 array with the given settings.
pub type Geometry<'a> = &'a dyn Fn(&Image, &Settings) -> Result<Image>;

/// Port of luma/range_mask.py `sample_selection`: up to five representative colours and the brightness span of a
/// point or rectangle (`points`, normalised in the displayed frame) as the layer `layer` sees its input (a new
/// layer, `None`, samples after the last existing layer). `source_shape` is the source photo's (h, w).
///
/// The settings are copied with the `masks` panel switched on; `develop` renders them and reports each layer's
/// input, `geometry` maps an all-ones source to the frame to find the valid (non-border) area.
pub fn sample_selection(
    source_shape: (usize, usize),
    settings: &Settings,
    points: &[[f64; 2]],
    layer: Option<i64>,
    develop: DevelopWithLayerInput<'_>,
    geometry: Geometry<'_>,
) -> Result<RangeSample> {
    if points.is_empty() || points.iter().flatten().any(|&v| !v.is_finite() || !(0.0..=1.0).contains(&v)) {
        return Err(invalid(BAD_POINTS));
    }
    let mut s = settings.clone();
    if !s.0.contains_key("masks") {
        s.0.insert("masks".into(), Value::Array(Vec::new()));
    }
    // Sampling works with the masks switched off; the new range switches them on again.
    let panels: Vec<Value> = match s.0.get("panels_off") {
        Some(Value::Array(list)) => list.iter().filter(|p| p.as_str() != Some("masks")).cloned().collect(),
        _ => Vec::new(),
    };
    s.0.insert("panels_off".into(), Value::Array(panels));
    let Some(Value::Array(layers)) = s.0.get_mut("masks") else {
        return Err(Error::BadValue { key: "masks".into(), found: settings.value("masks").to_string() });
    };
    let layer = match layer {
        None => {
            let mut new = Map::new();
            new.insert("components".into(), Value::Array(Vec::new()));
            new.insert("adjustments".into(), Value::Object(Map::new()));
            layers.push(Value::Object(new));
            layers.len() - 1
        }
        Some(index) if index >= 0 && (index as usize) < layers.len() => index as usize,
        Some(_) => return Err(invalid(NO_LAYER)),
    };
    let mut found: Option<Result<RangeSample>> = None;
    let rendered = {
        let s_ref = &s;
        let mut read = |index: usize, image: &Image| {
            if index != layer {
                return ControlFlow::Continue(());
            }
            let ones = Image::ones((source_shape.0, source_shape.1, 1));
            found = Some(geometry(&ones, &effective(s_ref)).and_then(|valid| select_colors(image, &valid, points)));
            ControlFlow::Break(())
        };
        develop(&s, &mut read)
    };
    match found {
        Some(result) => result,
        None => {
            rendered?;
            Err(invalid(NO_INPUT))
        }
    }
}
