//! Film negative to positive (port of `luma/negative.py`): each channel read as density above the clear film,
//! laid out over `range` stops of light.
//!
//! Rows of pixels (the oracle's `(n, 3)` arrays) are `Array2<f32>` with three columns.

use grainy_core::{settings::truthy, Image, Settings};
use ndarray::{s, Array2, ArrayView2, ArrayView3, Axis};
use serde_json::{json, Value};

use crate::npmath;
use crate::numeric::{median_f32, percentiles};
use crate::{Error, Result};

/// The settings of the conversion (`negative.KEYS`).
pub const KEYS: [&str; 5] = ["negative", "negative_base", "negative_peak", "negative_range", "negative_density"];
/// Scan values below this are treated as this (`negative.FLOOR`).
pub const FLOOR: f64 = 1e-5;
/// Share of each side left out of the analysis (`negative.BORDER`).
pub const BORDER: f64 = 0.06;
/// Clear film: the brightest part of the frame (`negative.BASE_PERCENT`).
pub const BASE_PERCENT: f64 = 99.7;
/// The densest part of the frame (`negative.PEAK_PERCENT`).
pub const PEAK_PERCENT: f64 = 0.3;
/// The measured density puts the picture's median here (`negative.MIDDLE`).
pub const MIDDLE: f64 = 0.18;
/// Limits of the measured density (`negative.DENSITY_RANGE`).
pub const DENSITY_RANGE: (f64, f64) = (-1.0, 2.5);
/// Pixels kept of each photo measured with others (`negative.SAMPLE`).
pub const SAMPLE: usize = 40000;

/// Port of `negative.DEFAULTS`.
pub fn defaults() -> serde_json::Map<String, Value> {
    let Value::Object(map) = json!({"negative": false, "negative_base": null, "negative_peak": null,
                                    "negative_range": 6.5, "negative_density": 0.0})
    else {
        unreachable!("a JSON object literal")
    };
    map
}

/// Port of `negative.inside`: the frame without its border (three channels).
pub fn inside(frame: &Image) -> ArrayView3<'_, f32> {
    let (height, width, channels) = frame.dim();
    let top = (height as f64 * BORDER) as usize;
    let left = (width as f64 * BORDER) as usize;
    let bottom = if height - top == 0 { height } else { height - top };
    let right = if width - left == 0 { width } else { width - left };
    frame.slice(s![top..bottom.max(top), left..right.max(left), ..channels.min(3)])
}

fn rows_of(view: ArrayView3<'_, f32>) -> Array2<f32> {
    let (h, w, _) = view.dim();
    let mut out = Array2::zeros((h * w, 3));
    for (mut row, pixel) in out.outer_iter_mut().zip(view.lanes(Axis(2))) {
        for c in 0..3 {
            row[c] = pixel[c];
        }
    }
    out
}

/// Port of `negative.analyse`: (base, peak), the scan values of the clear film and of the densest part.
pub fn analyse(source: &Image) -> ([f64; 3], [f64; 3]) {
    ends(rows_of(inside(source)).view())
}

/// Port of `negative.ends`: `analyse` for rows of pixels already inside the picture. The values are float32
/// numbers returned as Python floats.
pub fn ends(inner: ArrayView2<'_, f32>) -> ([f64; 3], [f64; 3]) {
    let limit = (FLOOR * 20.0) as f32;
    let floor = FLOOR as f32;
    let all: Vec<[f32; 3]> = inner.outer_iter().map(|r| [r[0], r[1], r[2]]).collect();
    let mut usable: Vec<[f32; 3]> = all.iter().copied().filter(|p| p.iter().all(|&v| v > limit)).collect();
    if usable.len() < 100 {
        usable = all;
    }
    let logs: Vec<[f32; 3]> =
        usable.iter().map(|p| p.map(|v| npmath::ln(if v.is_nan() { v } else { v.max(floor) }))).collect();
    let overall: Vec<f32> = logs.iter().map(|l| (l[0] + l[1] + l[2]) / 3.0).collect();
    let mut sorted = overall.clone();
    let q = percentiles(&mut sorted, &[BASE_PERCENT, PEAK_PERCENT]);
    let (clear, dense) = (q[0], q[1]);
    let median_of = |keep: &dyn Fn(f64) -> bool| -> [f32; 3] {
        [0, 1, 2].map(|c| {
            let mut column: Vec<f32> =
                logs.iter().zip(&overall).filter(|(_, &o)| keep(f64::from(o))).map(|(l, _)| l[c]).collect();
            npmath::exp(median_f32(&mut column))
        })
    };
    let base = median_of(&|o| o >= clear).map(|v| v.max((FLOOR * 10.0) as f32));
    let peak = median_of(&|o| o <= dense);
    let peak = [0, 1, 2].map(|c| peak[c].max(floor).min(base[c] * 0.9));
    (base.map(f64::from), peak.map(f64::from))
}

fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

const UNSUPPORTED: [&str; 12] = [
    "distortion",
    "distortion_k2",
    "distortion_k3",
    "perspective_h",
    "perspective_v",
    "aspect_scale",
    "shift_x",
    "shift_y",
    "ca_red",
    "ca_blue",
    "guides",
    "upright",
];

/// The part of `engine.geometry` a measurement needs: quarter turns, flip and crop. Lens corrections,
/// perspective, straightening and upright are not ported here (they belong to grainy-geometry) and give an
/// error.
fn geometry(a: &Image, s: &Settings) -> Result<Image> {
    let lens = s.bool("lensfun_enabled") && s.bool("lensfun");
    if lens || UNSUPPORTED.iter().any(|k| s.bool(k)) || s.f64("transform_scale") != 100.0 || s.f64("straighten") != 0.0
    {
        return Err(Error::Unsupported("negative measurement with lens, perspective or straighten geometry".into()));
    }
    let turns = (-s.i64("rotation")).rem_euclid(4);
    let mut a = a.view();
    for _ in 0..turns {
        // np.rot90(a, 1): out[i, j] = a[j, w - 1 - i]
        a.swap_axes(0, 1);
        a.invert_axis(Axis(0));
    }
    if s.bool("flip") {
        a.invert_axis(Axis(1));
    }
    let crop = s.value("crop");
    if truthy(crop) {
        let c: Vec<f64> =
            crop.as_array().map(|l| l.iter().map(|v| v.as_f64().unwrap_or(0.0)).collect()).unwrap_or_default();
        if c.len() != 4 {
            return Err(Error::Unsupported("crop must have four values".into()));
        }
        let (h, w) = (a.dim().0 as f64, a.dim().1 as f64);
        let round = |v: f64| v.round_ties_even();
        let l = (w - 1.0).min(0f64.max(round(c[0] * w)));
        let t = (h - 1.0).min(0f64.max(round(c[1] * h)));
        let r = (l + 1.0).max(w.min(round(c[2] * w)));
        let b = (t + 1.0).max(h.min(round(c[3] * h)));
        let view = a.slice(s![t as usize..b as usize, l as usize..r as usize, ..]);
        return Ok(view.to_owned());
    }
    Ok(a.to_owned())
}

/// Port of `negative.measured`: the part of `source` a measurement is made from, inside the crop of
/// `settings` without the border. The settings are normalized here, as the oracle does; a source is taken as
/// a decoded frame (a camera RAW stays in the camera's colours, which is what the loader gives).
pub fn measured(source: &Image, settings: &Settings) -> Result<Image> {
    let mut plain = grainy_core::normalized(&settings.0);
    plain.set("negative", true);
    let frame = geometry(source, &plain)?;
    Ok(inside(&frame).to_owned())
}

/// Port of `negative.sample`: `measured` as at most SAMPLE rows of pixels spread evenly over the picture.
pub fn sample(source: &Image, settings: &Settings) -> Result<Array2<f32>> {
    let part = measured(source, settings)?;
    let (h, w, _) = part.dim();
    let step = ((((h * w) as f64 / SAMPLE as f64).sqrt()).ceil() as usize).max(1);
    Ok(rows_of(part.slice(s![..;step, ..;step, ..])))
}

/// Port of `negative.converted`: settings with the conversion switched on, measured from `source` inside
/// the crop of `settings`.
pub fn converted(source: &Image, settings: &Settings) -> Result<Settings> {
    let pixels = rows_of(measured(source, settings)?.view());
    let (base, peak) = ends(pixels.view());
    Ok(placed(settings, pixels.view(), base, peak))
}

/// Port of `negative.converted_together`: `converted` for the photos of one roll, which share the clear
/// film and the densest part; brightness is placed photo by photo.
pub fn converted_together(samples: &[Array2<f32>], settings: &[Settings]) -> Vec<Settings> {
    let total: usize = samples.iter().map(Array2::nrows).sum();
    let mut all = Array2::zeros((total, 3));
    let mut at = 0;
    for part in samples {
        all.slice_mut(s![at..at + part.nrows(), ..]).assign(&part.slice(s![.., ..3]));
        at += part.nrows();
    }
    let (base, peak) = ends(all.view());
    samples.iter().zip(settings).map(|(pixels, one)| placed(one, pixels.view(), base, peak)).collect()
}

/// `raw_defaults.BUILTIN_RAW` and `BUILTIN_LINEAR_RAW`: (exposure, blacks).
const BUILTIN: [(f64, f64); 2] = [(1.5, -30.0), (0.75, -30.0)];

/// Port of `negative.placed`: `settings` converted with `base` and `peak`, with a first brightness from the
/// photo's own `pixels`.
pub fn placed(settings: &Settings, pixels: ArrayView2<'_, f32>, base: [f64; 3], peak: [f64; 3]) -> Settings {
    let mut result = settings.clone();
    result.set("negative", true);
    result.set("negative_base", base.to_vec());
    result.set("negative_peak", peak.to_vec());
    result.set("negative_density", 0.0);
    let n = pixels.nrows();
    let mut light: Vec<f32> = Vec::with_capacity(n);
    let conversion = Conversion::of(&result);
    for row in pixels.outer_iter() {
        let p = [row[0], row[1], row[2]];
        let v = conversion.as_ref().map_or(p, |c| c.pixel(p));
        light.push(v[0] * 0.2126 + v[1] * 0.7152 + v[2] * 0.0722);
    }
    let middle = f64::from(median_f32(&mut light));
    let density = (MIDDLE / middle.max(1e-4)).log2().clamp(DENSITY_RANGE.0, DENSITY_RANGE.1);
    result.set("negative_density", (density * 20.0).round_ties_even() / 20.0);
    let builtin = BUILTIN.iter().any(|&(exposure, blacks)| {
        number(settings.0.get("exposure")) == Some(exposure) && number(settings.0.get("blacks")) == Some(blacks)
    });
    if builtin {
        result.set("exposure", 0.0);
        result.set("blacks", 0.0);
    }
    result
}

/// Port of `negative.base_from`: the clear-film value from pixels the user pointed at.
pub fn base_from(sample: &Image) -> [f64; 3] {
    let (h, w, _) = sample.dim();
    [0, 1, 2].map(|c| {
        let mut column: Vec<f32> = Vec::with_capacity(h * w);
        column.extend(sample.slice(s![.., .., c]).iter().copied());
        f64::from(median_f32(&mut column).max((FLOOR * 10.0) as f32))
    })
}

/// The per-pixel constants of `apply`.
struct Conversion {
    log_base: [f32; 3],
    scale: [f32; 3],
    offset: f32,
    ceiling: f32,
}

impl Conversion {
    fn of(s: &Settings) -> Option<Self> {
        if !s.bool("negative") || s.is_none("negative_base") || s.is_none("negative_peak") {
            return None;
        }
        let list = |key: &str| -> [f32; 3] {
            let l = s.value(key).as_array();
            [0, 1, 2].map(|i| l.and_then(|l| l.get(i)).and_then(Value::as_f64).unwrap_or(f64::NAN) as f32)
        };
        let base = list("negative_base");
        let peak = list("negative_peak");
        let range = s.f64("negative_range");
        let density = s.f64("negative_density");
        let ln2 = std::f64::consts::LN_2;
        let range_ln2 = (range * ln2) as f32;
        let scale = [0, 1, 2].map(|c| {
            let p = if peak[c].is_nan() { peak[c] } else { peak[c].max(FLOOR as f32) };
            let limit = base[c] * 0.9f32;
            let lower = if p.is_nan() { p } else { p.min(limit) };
            range_ln2 / npmath::ln(base[c] / lower)
        });
        Some(Self {
            log_base: base.map(npmath::ln),
            scale,
            offset: ((density - range) * ln2) as f32,
            ceiling: (density * ln2) as f32,
        })
    }

    #[inline]
    fn pixel(&self, p: [f32; 3]) -> [f32; 3] {
        [0, 1, 2].map(|c| {
            let v = if p[c].is_nan() { p[c] } else { p[c].max(FLOOR as f32) };
            let y = (self.log_base[c] - npmath::ln(v)) * self.scale[c] + self.offset;
            npmath::exp(if y.is_nan() { y } else { y.min(self.ceiling) })
        })
    }
}

/// Port of `negative.apply`: the positive (scene light, linear) of linear frame `a`; a copy of `a` when the
/// conversion is off. Channels beyond three are kept.
pub fn apply(a: &Image, settings: &Settings) -> Image {
    let Some(conversion) = Conversion::of(settings) else {
        return a.clone();
    };
    let mut out = a.clone();
    if out.dim().2 < 3 {
        return out;
    }
    grainy_core::parallel::for_each_pixel(&mut out, |pixel| {
        let v = conversion.pixel([pixel[0], pixel[1], pixel[2]]);
        pixel[..3].copy_from_slice(&v);
    });
    out
}

/// Whether `apply` changes pixels with these settings (the oracle returns its input unchanged otherwise).
pub fn enabled(settings: &Settings) -> bool {
    Conversion::of(settings).is_some()
}
