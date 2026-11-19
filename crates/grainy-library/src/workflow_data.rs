//! The numerical part of `luma/workflow_data.py` that `grainy_catalog::workflow` does not have:
//! `automatic_white_balance`, `capture_exposure`, `match_exposure`. The maths keeps the oracle's float dtypes
//! (float32 arrays, float64 where numpy promotes).

use crate::pyformat::py_str;
use crate::{Error, Result};
use grainy_core::Settings;
use ndarray::Array3;
use serde_json::{Map, Value};

const MISSING: &str = "촬영 노출을 맞추려면 셔터·조리개·ISO 정보가 필요합니다.";
const INVALID: &str = "촬영 노출 정보가 올바르지 않습니다.";

/// `float(Fraction(text))` for the text forms `fractions.Fraction` accepts (`1/250`, `0.004`, `1.5e-3`, ...);
/// `None` where Python raises `ValueError` or `ZeroDivisionError`.
fn fraction_float(text: &str) -> Option<f64> {
    let t = text.trim();
    let (negative, rest) = match t.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let digits = |s: &str| -> Option<String> {
        // \d+(_\d+)*  (or empty)
        if s.is_empty() {
            return Some(String::new());
        }
        if s.starts_with('_')
            || s.ends_with('_')
            || s.contains("__")
            || !s.chars().all(|c| c.is_ascii_digit() || c == '_')
        {
            return None;
        }
        Some(s.replace('_', ""))
    };
    let lowered = rest.to_ascii_lowercase();
    let (body, exponent) = match lowered.split_once('e') {
        Some((b, e)) => (b.to_owned(), Some(e.to_owned())),
        None => (lowered.clone(), None),
    };
    let value = if let Some((n, d)) = body.split_once('/') {
        let (n, d) = (n.trim_end(), d.trim_start());
        if exponent.is_some() {
            return None;
        }
        let (n, d) = (digits(n)?, digits(d)?);
        if n.is_empty() || d.is_empty() {
            return None;
        }
        let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
        if d == 0.0 {
            return None; // ZeroDivisionError
        }
        n / d
    } else {
        let (whole, decimal) = match body.split_once('.') {
            Some((w, f)) => (digits(w)?, digits(f)?),
            None => (digits(&body)?, String::new()),
        };
        if whole.is_empty() && decimal.is_empty() {
            return None;
        }
        let exp = match exponent {
            Some(e) => {
                let (sign, d) = match e.strip_prefix('-') {
                    Some(r) => ("-", r),
                    None => ("", e.strip_prefix('+').unwrap_or(&e)),
                };
                let d = digits(d)?;
                if d.is_empty() {
                    return None;
                }
                format!("e{sign}{d}")
            }
            None => String::new(),
        };
        format!(
            "{}{}{}{exp}",
            if whole.is_empty() { "0" } else { &whole },
            if decimal.is_empty() { String::new() } else { format!(".{decimal}") },
            ""
        )
        .parse::<f64>()
        .ok()?
    };
    Some(if negative { -value } else { value })
}

fn number(info: &Value, key: &str) -> Result<f64> {
    let value = info.get(key).cloned().unwrap_or(Value::Null);
    let result = match &value {
        Value::Number(n) => n.as_f64(),
        Value::Null => None,
        other => fraction_float(&py_str(other)),
    };
    let Some(result) = result else { return Err(Error::value(MISSING)) };
    if !result.is_finite() || result <= 0.0 {
        return Err(Error::value(INVALID));
    }
    Ok(result)
}

/// Port of `workflow_data.capture_exposure`: the exposure value `log2(shutter * ISO / aperture^2)` of a photo's
/// metadata (`shutter`, `iso`, `aperture`; numbers or texts such as `1/250`).
pub fn capture_exposure(info: &Value) -> Result<f64> {
    let shutter = number(info, "shutter")?;
    let iso = number(info, "iso")?;
    let aperture = number(info, "aperture")?;
    Ok((shutter * iso / (aperture * aperture)).log2())
}

fn exposure_of(settings: &Value) -> Result<f64> {
    match settings.get("exposure") {
        Some(Value::Number(n)) => n.as_f64().ok_or_else(|| Error::Type("exposure is not a number".into())),
        Some(Value::Bool(b)) => Ok(f64::from(u8::from(*b))),
        Some(_) => Err(Error::Type("unsupported operand type(s) for +: 'float' and a non-number".into())),
        None => Err(Error::Type("KeyError: 'exposure'".into())),
    }
}

/// Port of `workflow_data.match_exposure`: the target's settings with `exposure` changed so that the target's
/// total exposure equals the reference's (clamped to -5..5 EV). Both records are catalogue rows
/// (`info` and `settings`).
pub fn match_exposure(reference: &Value, target: &Value) -> Result<Settings> {
    let ref_info = reference.get("info").ok_or_else(|| Error::Type("KeyError: 'info'".into()))?;
    let ref_settings = reference.get("settings").ok_or_else(|| Error::Type("KeyError: 'settings'".into()))?;
    let total = capture_exposure(ref_info)? + exposure_of(ref_settings)?;
    let target_settings = target.get("settings").ok_or_else(|| Error::Type("KeyError: 'settings'".into()))?;
    let mut result: Map<String, Value> =
        target_settings.as_object().cloned().ok_or_else(|| Error::Type("settings must be a dict".into()))?;
    let target_info = target.get("info").ok_or_else(|| Error::Type("KeyError: 'info'".into()))?;
    let value = (total - capture_exposure(target_info)?).clamp(-5.0, 5.0);
    result.insert("exposure".into(), serde_json::json!(value));
    Ok(Settings(result))
}

/// `np.percentile` of a sorted-on-the-fly array (linear method), computed as numpy does: virtual index
/// `(n-1)*q`, then `a + (b-a)*gamma`, or `b - (b-a)*(1-gamma)` from gamma 0.5 on.
pub fn percentile(values: &[f64], q: f64) -> f64 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();
    let quantile = q / 100.0;
    let virtual_index = (n as f64 - 1.0) * quantile;
    let previous = virtual_index.floor();
    let gamma = virtual_index - previous;
    let (previous, next) = if virtual_index >= n as f64 - 1.0 {
        (n - 1, n - 1)
    } else if virtual_index <= 0.0 {
        (0, 0)
    } else {
        (previous as usize, previous as usize + 1)
    };
    let (a, b) = (sorted[previous], sorted[next]);
    let diff = b - a;
    if gamma >= 0.5 {
        b - diff * (1.0 - gamma)
    } else {
        a + diff * gamma
    }
}

/// The oracle's `geometry(linear_frame(resize_float(source, 640), settings), settings)`.
pub type Prepare<'a> = &'a dyn Fn(&Array3<f32>, &Settings) -> Result<Array3<f32>>;

/// Port of `workflow_data.automatic_white_balance`. `prepare` is the oracle's
/// `geometry(linear_frame(resize_float(source, 640), settings), settings)` (it belongs to other crates); the
/// statistics that follow are ported here. The result is `settings` with `kelvin_enabled` off and a new
/// `temperature` and `tint`.
pub fn automatic_white_balance(source: &Array3<f32>, settings: &Settings, prepare: Prepare<'_>) -> Result<Settings> {
    let frame = prepare(source, settings)?;
    let (temperature, tint) = grainy_core::analysis::white_balance_statistics(&frame).map_err(Error::value)?;
    let mut values = settings.0.clone();
    values.insert("kelvin_enabled".into(), Value::Bool(false));
    values.insert("temperature".into(), serde_json::json!(temperature));
    values.insert("tint".into(), serde_json::json!(tint));
    Ok(Settings(values))
}
