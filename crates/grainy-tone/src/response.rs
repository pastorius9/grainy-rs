//! Port of luma/tone_response.py: Highlights and Shadows of tone process 3.
//!
//! The sliders act on a smooth base of the brightness (a guided filter on a reduced copy, stored as two
//! coefficient maps) and keep local contrast and colour. See the oracle module for the design notes.

use grainy_core::{Image, Plane};
use ndarray::{Array3, ArrayView3};

use crate::cv::{box_filter, LinearColumns};
use crate::interp::interp;
use crate::num::{clip, maximum, minimum, round_half_even};

/// `LIFT`: the largest lift that keeps a third of the base's contrast everywhere.
pub const LIFT: f64 = 2.0;
/// `REACH`: no pixel moves more than this share of its distance to black or white.
pub const REACH: f32 = 0.5;
/// `PIVOT`: 18% grey, sRGB-encoded.
pub const PIVOT: f64 = 0.4614;
/// `MAP_SIZE`: long edge of the coefficient maps.
pub const MAP_SIZE: usize = 512;
/// `RADIUS`: guided-filter radius as a fraction of the map's long edge.
pub const RADIUS: f64 = 0.02;
/// `EPS`: edges with less brightness variance than this are smoothed into the base.
pub const EPS: f64 = 0.02;
/// `FLOOR`: below about this luma the change turns from scaling into an offset.
pub const FLOOR: f64 = 0.05;
/// `WEIGHTS`: Rec. 709 luma weights (float32).
pub const WEIGHTS: [f32; 3] = [0.2126, 0.7152, 0.0722];
/// `len(X)`: samples of the change curve.
pub const SAMPLES: usize = 257;

/// The change curve over the base brightness `X = linspace(0, 1, 257)` (`curve()`'s result).
pub type Curve = [f32; SAMPLES];

/// `a @ WEIGHTS` of one pixel (NumPy's float32 matmul gives the sequential sum here).
#[inline]
pub(crate) fn luma(p: &[f32]) -> f32 {
    p[0] * WEIGHTS[0] + p[1] * WEIGHTS[1] + p[2] * WEIGHTS[2]
}

fn grid(n: usize) -> Vec<f64> {
    // np.linspace(0, 1, n): i * (1 / (n - 1)), the last one exactly 1 (exact here: n - 1 is a power of two).
    let step = 1.0 / (n - 1) as f64;
    let mut v: Vec<f64> = (0..n).map(|i| i as f64 * step).collect();
    v[n - 1] = 1.0;
    v
}

fn lift(t: f64, amount: f64) -> f64 {
    let u = 1.0 - t;
    t + amount * LIFT * t * (u * u)
}

/// `_side`: one side of the curve on t = 0 (black or white) .. 1 (pivot).
fn side(t: &[f64], amount: f64) -> Vec<f64> {
    if amount >= 0.0 {
        t.iter().map(|&x| lift(x, amount)).collect()
    } else {
        let fine = grid(2049);
        let lifted: Vec<f64> = fine.iter().map(|&x| lift(x, -amount)).collect();
        t.iter().map(|&x| interp(x, &lifted, &fine)).collect()
    }
}

/// Port of luma/tone_response.py `curve`: the brightness change as a function of the base brightness, float32.
pub fn curve(highlights: f64, shadows: f64) -> Curve {
    let pivot = PIVOT;
    let shadows = shadows.clamp(-100.0, 100.0) / 100.0;
    let highlights = highlights.clamp(-100.0, 100.0) / 100.0;
    let x = grid(SAMPLES);
    let below_t: Vec<f64> = x.iter().map(|&v| minimum(v / pivot, 1.0)).collect();
    let above_t: Vec<f64> = x.iter().map(|&v| minimum((1.0 - v) / (1.0 - pivot), 1.0)).collect();
    let below = side(&below_t, shadows);
    let above = side(&above_t, -highlights);
    let mut out = [0f32; SAMPLES];
    for i in 0..SAMPLES {
        let value = if x[i] < pivot { pivot * below[i] } else { 1.0 - (1.0 - pivot) * above[i] };
        out[i] = (value - x[i]) as f32;
    }
    out
}

/// The coefficient maps of `analyse()`: base = a·luma + b, at the map's size, for a frame of `shape`.
#[derive(Clone, Debug, PartialEq)]
pub struct ToneBase {
    /// The `a` map (float32).
    pub a: Plane,
    /// The `b` map (float32).
    pub b: Plane,
    /// The frame's (height, width).
    pub shape: (usize, usize),
}

/// `with_curve()`'s result: what `apply()` takes.
#[derive(Clone, Debug, PartialEq)]
pub struct ToneLocal {
    /// The coefficient maps.
    pub base: ToneBase,
    /// The change curve for the slider values.
    pub curve: Curve,
}

/// Port of luma/tone_response.py `analyse`: coefficient maps (a guided filter) of a reduced sRGB-encoded copy of
/// the frame; `shape` is the frame's (height, width).
pub fn analyse(encoded: &Image, shape: (usize, usize)) -> ToneBase {
    let (h, w, _) = encoded.dim();
    let mut luma_plane = Plane::zeros((h, w));
    for ((y, x), slot) in luma_plane.indexed_iter_mut() {
        let p = [encoded[[y, x, 0]], encoded[[y, x, 1]], encoded[[y, x, 2]]];
        *slot = clip(luma(&p), 0.0, 1.0);
    }
    let r = (round_half_even(RADIUS * h.max(w) as f64) as usize).max(1);
    let mean = box_filter(&luma_plane, r);
    let squares = luma_plane.mapv(|v| v * v);
    let mean_sq = box_filter(&squares, r);
    let eps = EPS as f32;
    let mut a = Plane::zeros((h, w));
    let mut b = Plane::zeros((h, w));
    for ((index, m), s) in mean.indexed_iter().zip(mean_sq.iter()) {
        let variance = s - m * m;
        let av = variance / (variance + eps);
        a[index] = av;
        b[index] = m - av * m;
    }
    ToneBase { a: box_filter(&a, r), b: box_filter(&b, r), shape }
}

/// Port of luma/tone_response.py `with_curve`.
pub fn with_curve(base: ToneBase, highlights: f64, shadows: f64) -> ToneLocal {
    ToneLocal { base, curve: curve(highlights, shadows) }
}

/// One frame row of a coefficient map enlarged bilinearly (`_band`), float64.
struct Rows<'a> {
    map: &'a Plane,
    height: usize,
    columns: Option<LinearColumns>,
}

impl<'a> Rows<'a> {
    fn new(map: &'a Plane, shape: (usize, usize)) -> Self {
        let (_, cols) = map.dim();
        let columns = (cols != shape.1).then(|| LinearColumns::new(cols, shape.1));
        Self { map, height: shape.0, columns }
    }

    fn row(&self, y: usize, out: &mut [f64]) {
        let (rows, cols) = self.map.dim();
        let scale = (rows as f64 / self.height as f64) as f32;
        let fy = clip((y as f32 + 0.5) * scale - 0.5, 0.0, (rows - 1) as f32);
        let upper = fy.floor() as usize;
        let lower = (upper + 1).min(rows - 1);
        let t = f64::from(fy) - upper as f64;
        let band: Vec<f64> = (0..cols)
            .map(|x| f64::from(self.map[[upper, x]]) * (1.0 - t) + f64::from(self.map[[lower, x]]) * t)
            .collect();
        match &self.columns {
            Some(table) => table.row(&band, out),
            None => out.copy_from_slice(&band),
        }
    }
}

/// Port of luma/tone_response.py `apply`: Highlights/Shadows on sRGB-encoded rows `first..first+len(a)` of the
/// frame that `local` describes. The oracle's result is float64 (its interpolation weights are float64).
pub fn apply(a: ArrayView3<'_, f32>, local: &ToneLocal, first: usize) -> Array3<f64> {
    let (count, width, channels) = a.dim();
    let mut out = Array3::<f64>::zeros((count, width, channels));
    let shape = local.base.shape;
    let same = local.base.a.dim() == shape;
    let rows_a = Rows::new(&local.base.a, shape);
    let rows_b = Rows::new(&local.base.b, shape);
    let curve = &local.curve;
    let floor32 = FLOOR as f32;
    let floor = f64::from(floor32);
    let last = (SAMPLES - 1) as f32;
    let mut band_a = vec![0f64; width];
    let mut band_b = vec![0f64; width];
    for i in 0..count {
        let y = first + i;
        if !same {
            rows_a.row(y, &mut band_a);
            rows_b.row(y, &mut band_b);
        }
        for x in 0..width {
            let p = [a[[i, x, 0]], a[[i, x, 1]], a[[i, x, 2]]];
            let l = luma(&p);
            let position = if same {
                let base = local.base.a[[y, x]] * l + local.base.b[[y, x]];
                f64::from(clip(base, 0.0, 1.0) * last)
            } else {
                let base = band_a[x] * f64::from(l) + band_b[x];
                clip(base, 0.0, 1.0) * f64::from(last)
            };
            let index = (position as usize).min(SAMPLES - 2);
            let t = position - index as f64;
            let change = f64::from(curve[index]) * (1.0 - t) + f64::from(curve[index + 1]) * t;
            let low = -maximum(l, 0.0) * REACH;
            let high = maximum(1.0 - l, 0.0) * REACH;
            let change = clip(change, f64::from(low), f64::from(high));
            let target = maximum(f64::from(l) + change, 0.0);
            let l64 = f64::from(l);
            let denominator = f64::from(l + floor32);
            for c in 0..channels {
                let v = f64::from(a[[i, x, c]]);
                out[[i, x, c]] = (v * target + floor * (v + target - l64)) / denominator;
            }
        }
    }
    out
}
