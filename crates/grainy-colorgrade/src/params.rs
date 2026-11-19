//! Reading the colour stage's settings (`COLOR_KEYS` of `luma/engine.py`) into plain Rust values.

use grainy_core::Settings;
use serde_json::{Map, Value};

use crate::{Error, Result};

/// A JSON number as Python sees it in arithmetic (`bool` is 0/1).
pub(crate) fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        _ => None,
    }
}

fn bad(key: &str, value: &Value) -> Error {
    Error::BadValue { key: key.to_owned(), found: value.to_string() }
}

/// A list of numbers (`[h, s, l]`, `parametric`).
fn numbers(key: &str, value: &Value) -> Result<Vec<f64>> {
    value.as_array().ok_or_else(|| bad(key, value))?.iter().map(|v| number(v).ok_or_else(|| bad(key, value))).collect()
}

/// A list of lists of `size` numbers (`hsl`, `calibration`, `grading`); the oracle unpacks each group, so another
/// size is an error.
fn groups(key: &str, value: &Value, size: usize) -> Result<Vec<Vec<f64>>> {
    let list = value.as_array().ok_or_else(|| bad(key, value))?;
    list.iter()
        .map(|group| {
            let values = numbers(key, group)?;
            if values.len() == size {
                Ok(values)
            } else {
                Err(bad(key, group))
            }
        })
        .collect()
}

/// The colour stage's settings, read once.
#[derive(Clone, Debug)]
pub struct ColorParams {
    /// `saturation` (-100..100).
    pub saturation: f64,
    /// `vibrance` (-100..100).
    pub vibrance: f64,
    /// `mixer_mode`: `"hsl"` (OpenCV HLS mixer) or anything else (the HSV mixer).
    pub mixer_mode: String,
    /// `hsl`: up to 8 `[hue, saturation, luminance]` bands.
    pub hsl: Vec<Vec<f64>>,
    /// `rgb_curves`: per channel, the curve points as float32 `(x, y)`.
    pub rgb_curves: Vec<Vec<[f32; 2]>>,
    /// `parametric`: shadows, darks, lights, highlights.
    pub parametric: Vec<f64>,
    /// `calibration`: red, green, blue primaries `[hue, saturation]`.
    pub calibration: Vec<Vec<f64>>,
    /// `point_colors`: the point colour dicts.
    pub point_colors: Vec<Map<String, Value>>,
    /// `grading`: shadows, midtones, highlights `[hue, saturation, lightness]`.
    pub grading: Vec<Vec<f64>>,
    /// `grading_balance`.
    pub grading_balance: f64,
    /// `camera_matrix` when it is a finite float32 3×3 matrix (the oracle ignores any other shape).
    pub camera_matrix: Option<[[f32; 3]; 3]>,
    /// `hdr` (the shipped oracle normalizes it to false).
    pub hdr: bool,
}

fn camera_matrix(value: &Value) -> Option<[[f32; 3]; 3]> {
    let rows = value.as_array()?;
    if rows.len() != 3 {
        return None;
    }
    let mut matrix = [[0f32; 3]; 3];
    for (i, row) in rows.iter().enumerate() {
        let row = row.as_array()?;
        if row.len() != 3 {
            return None;
        }
        for (j, v) in row.iter().enumerate() {
            let x = number(v)? as f32;
            if !x.is_finite() {
                return None;
            }
            matrix[i][j] = x;
        }
    }
    Some(matrix)
}

impl ColorParams {
    /// Reads the colour keys of normalized settings.
    pub fn from_settings(s: &Settings) -> Result<Self> {
        let curves_value = s.value("rgb_curves");
        let mut rgb_curves = Vec::new();
        for curve in curves_value.as_array().ok_or_else(|| bad("rgb_curves", curves_value))? {
            let mut points = Vec::new();
            for point in curve.as_array().ok_or_else(|| bad("rgb_curves", curve))? {
                let xy = numbers("rgb_curves", point)?;
                if xy.len() < 2 {
                    return Err(bad("rgb_curves", point));
                }
                points.push([xy[0] as f32, xy[1] as f32]);
            }
            if points.is_empty() {
                return Err(bad("rgb_curves", curve));
            }
            rgb_curves.push(points);
        }
        let points_value = s.value("point_colors");
        let point_colors = match points_value {
            Value::Null => Vec::new(),
            Value::Array(list) => list
                .iter()
                .map(|p| p.as_object().cloned().ok_or_else(|| bad("point_colors", p)))
                .collect::<Result<_>>()?,
            other => return Err(bad("point_colors", other)),
        };
        Ok(Self {
            saturation: s.f64("saturation"),
            vibrance: s.f64("vibrance"),
            mixer_mode: s.str("mixer_mode").to_owned(),
            hsl: groups("hsl", s.value("hsl"), 3)?,
            rgb_curves,
            parametric: numbers("parametric", s.value("parametric"))?,
            calibration: groups("calibration", s.value("calibration"), 2)?,
            point_colors,
            grading: groups("grading", s.value("grading"), 3)?,
            grading_balance: s.f64("grading_balance"),
            camera_matrix: camera_matrix(s.value("camera_matrix")),
            hdr: s.bool("hdr"),
        })
    }
}

/// `any(any(group) for group in groups)`.
pub(crate) fn any_set(groups: &[Vec<f64>]) -> bool {
    groups.iter().any(|g| g.iter().any(|&x| x != 0.0))
}
