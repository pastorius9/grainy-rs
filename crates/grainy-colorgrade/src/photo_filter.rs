//! Port of `luma/photo_filter.py`: art-directed RGB approximations of photographic filters.

use grainy_core::{Image, Settings};
use serde_json::Value;

use crate::convert::{clip01, dot3, f, np_max, np_min, to_linear_value, to_srgb_value, LUMA};
use crate::tools::per_pixel;

/// `FILTERS`: stable id, Korean label and linear RGB transmission at full density.
pub const FILTERS: [(&str, &str, [f64; 3]); 17] = [
    ("warming85", "(85) 웜 필터", [1., 0.58, 0.22]),
    ("warming81", "(81) 웜 필터", [1., 0.88, 0.64]),
    ("cooling80", "(80) 쿨 필터", [0.28, 0.57, 1.]),
    ("cooling82", "(82) 쿨 필터", [0.69, 0.87, 1.]),
    ("red", "레드", [1., 0.12, 0.08]),
    ("orange", "오렌지", [1., 0.43, 0.05]),
    ("yellow", "옐로", [1., 0.93, 0.07]),
    ("green", "그린", [0.12, 1., 0.18]),
    ("cyan", "시안", [0.10, 1., 1.]),
    ("blue", "블루", [0.10, 0.22, 1.]),
    ("violet", "바이올렛", [0.50, 0.12, 1.]),
    ("magenta", "마젠타", [1., 0.10, 0.70]),
    ("dark_red", "다크 레드", [0.60, 0.05, 0.06]),
    ("navy_blue", "네이비 블루", [0.06, 0.12, 0.58]),
    ("emerald_green", "에메랄드 그린", [0.06, 0.65, 0.32]),
    ("chrome_yellow", "크롬 옐로", [1., 0.66, 0.04]),
    ("marine_blue", "마린 블루", [0.05, 0.48, 0.80]),
];

/// `KEYS`: the settings the filter reads.
pub const KEYS: [&str; 4] = ["photo_filter", "photo_filter_enabled", "photo_filter_density", "photo_filter_luminosity"];

/// `TRANSMISSION[key]`.
pub fn transmission(key: &str) -> Option<[f64; 3]> {
    FILTERS.iter().find(|(id, _, _)| *id == key).map(|(_, _, rgb)| *rgb)
}

/// `gain @ weights` of two float32 3-vectors as numpy's BLAS (OpenBLAS `sdot`) computes it: float32 products
/// summed in float64.
fn sdot3(a: &[f32; 3], b: &[f32; 3]) -> f32 {
    (f64::from(a[0] * b[0]) + f64::from(a[1] * b[1]) + f64::from(a[2] * b[2])) as f32
}

/// Python's `float(value)` for a settings value, `None` where it would raise.
fn python_float(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::String(text) => {
            let t = text.trim().to_ascii_lowercase().replace('_', "");
            match t.as_str() {
                "nan" | "+nan" | "-nan" => Some(f64::NAN),
                "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
                "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
                _ => t.parse().ok(),
            }
        }
        _ => None,
    }
}

/// Port of `luma/photo_filter.py apply`: the filter on display-encoded RGB (to linear, filter, back to sRGB), with
/// "preserve luminosity" keeping each pixel's luminance (and fitting highlights around it), or for monochrome
/// compensating neutral grey and returning grey.
pub fn apply(image: &Image, s: &Settings) -> Image {
    if !s.bool("photo_filter_enabled") {
        return image.clone();
    }
    let Some(rgb) = transmission(s.str("photo_filter")) else { return image.clone() };
    let density = match s.0.get("photo_filter_density") {
        None => 25.0 / 100.0,
        Some(value) => match python_float(value) {
            Some(x) => x / 100.0,
            None => return image.clone(),
        },
    };
    if !density.is_finite() || density <= 0.0 {
        return image.clone();
    }
    let amount = density.min(1.0) as f32;
    let gain = [1.0 + amount * (f(rgb[0]) - 1.0), 1.0 + amount * (f(rgb[1]) - 1.0), 1.0 + amount * (f(rgb[2]) - 1.0)];
    let luminosity = s.0.get("photo_filter_luminosity").is_none_or(grainy_core::settings::truthy);
    let weights = if s.0.get("working_space").and_then(Value::as_str) == Some("ProPhoto") {
        [f(0.28804), f(0.71187), f(0.00009)]
    } else {
        LUMA
    };
    let monochrome = s.0.get("monochrome").is_some_and(grainy_core::settings::truthy);
    let neutral = sdot3(&gain, &weights);
    per_pixel(image, |_, _, px| {
        let linear = px.map(to_linear_value);
        let mut filtered = [linear[0] * gain[0], linear[1] * gain[1], linear[2] * gain[2]];
        if luminosity {
            if monochrome {
                let compensated = filtered.map(|v| v / neutral);
                filtered = [dot3(&compensated, &weights); 3];
            } else {
                let luminance = dot3(&linear, &weights);
                let changed = dot3(&filtered, &weights);
                let ratio = luminance / np_max(changed, f(1e-12));
                filtered = filtered.map(|v| v * ratio);
                let delta = filtered.map(|v| v - luminance);
                let upper = (1.0 - luminance) / np_max(np_max(np_max(delta[0], delta[1]), delta[2]), f(1e-12));
                let lower = luminance / np_max(-np_min(np_min(delta[0], delta[1]), delta[2]), f(1e-12));
                let amount = clip01(np_min(upper, lower));
                filtered = delta.map(|d| luminance + d * amount);
            }
        }
        filtered.map(|v| to_srgb_value(np_max(v, 0.0)))
    })
}
