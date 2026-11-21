//! Port of luma/engine.py `geometry`.

use std::borrow::Cow;

use grainy_core::Settings;
use ndarray::{s, Array3};
use serde_json::Value;

use crate::lens::LensModel;
use crate::numeric::py_round;
use crate::optical::optical_geometry;
use crate::orient::{flip_left_right, rot90};
use crate::pillow::rotate_bicubic;
use crate::{straighten, upright, Error, Result};

/// The oracle's `use_crop` argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseCrop {
    /// `True`: apply `settings['crop']`.
    Yes,
    /// `False`: the uncropped frame.
    No,
    /// `'all'`: the crop tool's view, uncropped on a canvas enlarged so a straightened photo's corners show.
    All,
}

fn crop_values(value: &Value) -> Result<[f64; 4]> {
    let bad = || Error::Value("crop must be four numbers".into());
    let items = value.as_array().ok_or_else(bad)?;
    if items.len() != 4 {
        return Err(bad());
    }
    let mut out = [0.0; 4];
    for (slot, v) in out.iter_mut().zip(items) {
        *slot = match v {
            Value::Number(n) => n.as_f64().ok_or_else(bad)?,
            Value::Bool(b) => f64::from(u8::from(*b)),
            _ => return Err(bad()),
        };
    }
    Ok(out)
}

/// Python's `min(a, b)` / `max(a, b)` on integers.
fn clamp_index(v: f64, low: i64, high: i64) -> i64 {
    (v as i64).max(low).min(high)
}

/// Pixel bounds of a stored crop on the rotated canvas, with the same rounding as the renderer.
pub fn crop_bounds(crop: &Value, w: usize, h: usize) -> Result<[usize; 4]> {
    let [x0, y0, x1, y1] = crop_values(crop)?;
    if h == 0 || w == 0 {
        return Err(Error::Value("cannot crop an empty frame".into()));
    }
    let (wi, hi) = (w as i64, h as i64);
    let l = clamp_index(py_round(x0 * w as f64), 0, wi - 1);
    let t = clamp_index(py_round(y0 * h as f64), 0, hi - 1);
    let r = (py_round(x1 * w as f64) as i64).min(wi).max(l + 1);
    let b = (py_round(y1 * h as f64) as i64).min(hi).max(t + 1);
    Ok([l as usize, t as usize, r as usize, b as usize])
}

/// Port of luma/engine.py `geometry(a, settings, use_crop)` on an H×W×C frame or mask (1 or 3 channels):
/// optical geometry (lens profile, guides, manual transform), quarter turns (`rotation`), `flip`, the crop
/// tool's padded canvas (`UseCrop::All`), straighten (Pillow bicubic per channel, the oracle's CPU path),
/// upright, then the crop. Returns the input borrowed when no geometry applies (the oracle returns a copy,
/// or with `copy=False` a read-only view: the same pixels).
pub fn geometry<'a>(
    a: &'a Array3<f32>,
    settings: &Settings,
    use_crop: UseCrop,
    lens: Option<&dyn LensModel>,
) -> Result<Cow<'a, Array3<f32>>> {
    let mut a = optical_geometry(a, settings, lens)?;
    let turns = (-settings.i64("rotation")).rem_euclid(4);
    if turns != 0 {
        a = Cow::Owned(rot90(a.view(), turns));
    }
    if settings.bool("flip") {
        a = Cow::Owned(flip_left_right(a.view()));
    }
    let angle = settings.f64("straighten");
    if angle != 0.0 && use_crop == UseCrop::All && !settings.bool("upright") {
        let (h, w, c) = a.dim();
        let (pad_x, pad_y) = straighten::padding(angle, w, h);
        if pad_x != 0 || pad_y != 0 {
            let mut padded = Array3::<f32>::zeros((h + 2 * pad_y, w + 2 * pad_x, c));
            padded.slice_mut(s![pad_y..pad_y + h, pad_x..pad_x + w, ..]).assign(&a);
            a = Cow::Owned(padded);
        }
    }
    if angle != 0.0 {
        a = Cow::Owned(rotate_bicubic(a.view(), -angle));
    }
    if settings.bool("upright") {
        a = match a {
            Cow::Borrowed(b) => upright::apply(b, settings)?,
            Cow::Owned(o) => Cow::Owned(upright::apply(&o, settings)?.into_owned()),
        };
    }
    let crop = if use_crop == UseCrop::Yes { settings.value("crop") } else { &Value::Null };
    if grainy_core::settings::truthy(crop) {
        let (h, w, _) = a.dim();
        let [l, t, r, b] = crop_bounds(crop, w, h)?;
        let cropped = a.slice(s![t..b, l..r, ..]).to_owned();
        a = Cow::Owned(cropped);
    }
    Ok(a)
}
