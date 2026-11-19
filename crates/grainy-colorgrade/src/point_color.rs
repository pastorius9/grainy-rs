//! Port of `luma/point_color.py`: point colours (colour selections in working RGB shifted in OpenCV's HLS).
//!
//! `sample`, `preview` and `preview_key` of the oracle run the whole `develop` to reach the input of a point; they
//! belong to the engine crate. What they compute on that input is here: [`weights`] / [`coverage`].

use grainy_core::{Image, Plane};
use ndarray::{Array2, Axis};
use serde_json::{Map, Value};

use crate::convert::{clip01, f, hsl_to_rgb_pixel, py_mod, rgb_to_hsl_pixel, simd_column};
use crate::params::number;
use crate::tools::per_pixel;
use crate::{Error, Result};

/// The point-colour picker hook (`on_point_input` of the oracle): called with the point index and the image that
/// point works on, and once more after the last point with `points.len()` and the result.
pub type PointHook<'a> = &'a mut dyn FnMut(usize, &Image);

const BAD_COLOUR: &str = "선택 색상이 올바르지 않습니다.";
const BAD_RANGE: &str = "선택 색상 범위가 올바르지 않습니다.";

fn get(point: &Map<String, Value>, key: &str, default: f64) -> Result<f64> {
    match point.get(key) {
        None => Ok(default),
        Some(value) => number(value).ok_or_else(|| Error::BadValue { key: key.to_owned(), found: value.to_string() }),
    }
}

fn reference_rgb(point: &Map<String, Value>) -> Result<[f32; 3]> {
    let Some(value) = point.get("rgb") else { return Ok([0.5; 3]) };
    let list = value.as_array().ok_or(Error::InvalidPoint(BAD_COLOUR))?;
    if list.len() != 3 {
        return Err(Error::InvalidPoint(BAD_COLOUR));
    }
    let mut rgb = [0f32; 3];
    for (slot, v) in rgb.iter_mut().zip(list) {
        *slot = number(v).ok_or(Error::InvalidPoint(BAD_COLOUR))? as f32;
    }
    Ok(rgb)
}

/// The reference colour's (hue, saturation, lightness): `rgb_to_hsl` of a 1×1 image, so OpenCV's scalar code.
fn reference_hsl(rgb: [f32; 3]) -> (f32, f32, f32) {
    rgb_to_hsl_pixel(&rgb, false)
}

/// The selection of a version-2 point, prepared once.
struct Selection {
    rh: f32,
    rs: f32,
    rl: f32,
    widths: [f32; 3],
    all_hues: bool,
}

impl Selection {
    fn new(point: &Map<String, Value>) -> Result<Self> {
        let rgb = reference_rgb(point)?;
        if !rgb.iter().all(|v| v.is_finite()) {
            return Err(Error::InvalidPoint(BAD_COLOUR));
        }
        let (rh, rs, rl) = reference_hsl(rgb);
        let widths = [
            get(point, "range", 0.12)? as f32,
            get(point, "saturation_range", 0.6)? as f32,
            get(point, "lightness_range", 0.5)? as f32,
        ];
        let limits = [0.5f32, 1.0, 1.0];
        if widths.iter().zip(limits).any(|(&w, limit)| !w.is_finite() || w <= 0.0 || w > limit) {
            return Err(Error::InvalidPoint(BAD_RANGE));
        }
        Ok(Self { rh, rs, rl, widths, all_hues: rs < f(0.02) || widths[0] >= 0.5 })
    }

    #[inline]
    fn weight(&self, h: f32, s: f32, l: f32) -> f32 {
        let hue = if self.all_hues {
            1.0
        } else {
            clip01(1.0 - (py_mod(h - self.rh + 0.5, 1.0) - 0.5).abs() / self.widths[0])
        };
        let saturation = if self.widths[1] >= 1.0 { 1.0 } else { clip01(1.0 - (s - self.rs).abs() / self.widths[1]) };
        let lightness = if self.widths[2] >= 1.0 { 1.0 } else { clip01(1.0 - (l - self.rl).abs() / self.widths[2]) };
        hue * saturation * lightness
    }
}

/// The selection of a version-1 point (hue width and a fixed .6 saturation width).
struct LegacySelection {
    rh: f32,
    rs: f32,
    width: f32,
}

impl LegacySelection {
    fn new(point: &Map<String, Value>) -> Result<Self> {
        let (rh, rs, _) = reference_hsl(reference_rgb(point)?);
        Ok(Self { rh, rs, width: get(point, "range", 0.12)?.max(0.005) as f32 })
    }

    #[inline]
    fn weight(&self, h: f32, s: f32) -> f32 {
        let w = clip01(1.0 - (py_mod(h - self.rh + 0.5, 1.0) - 0.5).abs() / self.width);
        w * clip01(1.0 - (s - self.rs).abs() / f(0.6))
    }
}

fn version(point: &Map<String, Value>) -> Result<f64> {
    get(point, "version", 1.0)
}

fn plane_of<F>(image: &Image, weight: F) -> Plane
where
    F: Fn(f32, f32, f32) -> f32,
{
    let (height, width, _) = image.dim();
    let mut out = Array2::<f32>::zeros((height, width));
    for (y, row) in image.axis_iter(Axis(0)).enumerate() {
        for (x, p) in row.axis_iter(Axis(0)).enumerate() {
            let (h, s, l) = rgb_to_hsl_pixel(&[p[0], p[1], p[2]], simd_column(x, width));
            out[[y, x]] = weight(h, s, l);
        }
    }
    out
}

/// Port of `luma/point_color.py weights`: the selection weight of a version-2 point on `image`.
pub fn weights(image: &Image, point: &Map<String, Value>) -> Result<Plane> {
    let selection = Selection::new(point)?;
    Ok(plane_of(image, |h, s, l| selection.weight(h, s, l)))
}

/// The coverage `luma/point_color.py preview` draws for a point (version 2: [`weights`]; version 1: its hue and
/// saturation weight), before the mask and the placement in the frame.
pub fn coverage(image: &Image, point: &Map<String, Value>) -> Result<Plane> {
    if version(point)? >= 2.0 {
        weights(image, point)
    } else {
        let selection = LegacySelection::new(point)?;
        Ok(plane_of(image, |h, s, _| selection.weight(h, s)))
    }
}

/// Port of `luma/point_color.py apply`: the point colours in order. Version-2 points change only the selected
/// pixels; version-1 points send the whole image through the HLS round trip.
pub fn apply(image: Image, points: &[Map<String, Value>], mut on_input: Option<PointHook<'_>>) -> Result<Image> {
    let mut a = image;
    for (index, point) in points.iter().enumerate() {
        if let Some(hook) = on_input.as_mut() {
            hook(index, &a);
        }
        let hue = get(point, "hue", 0.0)?;
        let saturation = get(point, "saturation", 0.0)?;
        let lightness = get(point, "lightness", 0.0)?;
        let (hue32, sat32, light32) = (hue as f32, saturation as f32, lightness as f32);
        let shift = move |h: f32, s: f32, l: f32, w: f32, simd: bool| {
            hsl_to_rgb_pixel(h + w * hue32 / 600.0, s * (1.0 + w * sat32 / 100.0), l + w * light32 / 200.0, simd)
        };
        if version(point)? >= 2.0 {
            if hue == 0.0 && saturation == 0.0 && lightness == 0.0 {
                continue;
            }
            let selection = Selection::new(point)?;
            a = per_pixel(&a, |x, width, px| {
                let simd = simd_column(x, width);
                let (h, s, l) = rgb_to_hsl_pixel(&px, simd);
                let w = selection.weight(h, s, l);
                if w > 0.0 {
                    shift(h, s, l, w, simd)
                } else {
                    px
                }
            });
        } else {
            let selection = LegacySelection::new(point)?;
            a = per_pixel(&a, |x, width, px| {
                let simd = simd_column(x, width);
                let (h, s, l) = rgb_to_hsl_pixel(&px, simd);
                shift(h, s, l, selection.weight(h, s), simd)
            });
        }
    }
    if let Some(hook) = on_input.as_mut() {
        hook(points.len(), &a);
    }
    Ok(a)
}
