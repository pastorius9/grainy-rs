//! Port of luma/tone_mapping.py: the smooth-highlights display rendering (DRT), chosen instead of the classic
//! roll-off. One tone scale m·(x/(x+s))^p on the largest channel (hue kept), a toe or flare, and a path to white.

use grainy_core::{Image, Settings};
use serde_json::Value;

use crate::num::{clip, maximum, minimum, pixels_mut};

/// `MID`: mid grey stays mid grey.
pub const MID: f64 = 0.18;
/// `CONTRAST`: p, the curve's slope in deep shadows.
pub const CONTRAST: f64 = 1.35;
/// `WHITE_START`: display level where colours start moving towards white.
pub const WHITE_START: f64 = 0.8;
/// `KEYS`: the look's settings.
pub const KEYS: [&str; 4] = ["drt_contrast", "drt_toe", "drt_shoulder", "drt_white"];
/// `LOOKS`: named looks for the Basic panel, values of `KEYS` ('표준' is the original rendering).
pub const LOOKS: [(&str, [f64; 4]); 4] = [
    ("표준", [0.0, 0.0, 0.0, 0.0]),
    ("부드럽게", [-35.0, -25.0, -40.0, 0.0]),
    ("대비 강하게", [40.0, 30.0, 30.0, 0.0]),
    ("필름처럼", [15.0, 35.0, -20.0, -45.0]),
];

/// Port of `tone_mapping.Params`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    /// Tone-scale exponent.
    pub p: f64,
    /// Tone-scale offset, solved so mid grey lands on 0.18 after the toe.
    pub s: f64,
    /// Tone-scale gain (above 1: white at a finite exposure).
    pub m: f64,
    /// Toe: positive deepens, negative is a flare.
    pub toe: f64,
    /// Display level where colours start moving towards white.
    pub white: f64,
    /// Longer shoulder (≤ 0).
    pub shoulder: f64,
}

/// Python's `float(value)` of a settings value, `None` when it raises.
pub(crate) fn python_float(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::String(s) => {
            let t = s.trim();
            match t.to_ascii_lowercase().as_str() {
                "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
                "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
                "nan" | "+nan" | "-nan" => Some(f64::NAN),
                _ => t.parse::<f64>().ok(),
            }
        }
        _ => None,
    }
}

/// `settings.get(key, 0.) or 0.`, then `float()`; `None` when `float()` raises.
fn setting(settings: &Settings, key: &str) -> Option<f64> {
    let value = settings.value(key);
    if !grainy_core::settings::truthy(value) {
        return Some(0.0);
    }
    python_float(value)
}

/// `_value`: a look setting as -1..1 (0 for values that are not numbers).
fn value(settings: &Settings, key: &str) -> f64 {
    setting(settings, key).map_or(0.0, |v| v.clamp(-100.0, 100.0) / 100.0)
}

fn untoe(y: f64, toe: f64) -> f64 {
    if toe > 0.0 {
        (y + (y * y + 4.0 * (1.0 + toe) * y * toe).sqrt()) / (2.0 * (1.0 + toe))
    } else if toe < 0.0 {
        (y + toe) / (1.0 + toe)
    } else {
        y
    }
}

/// Port of luma/tone_mapping.py `params`.
pub fn params(settings: &Settings) -> Params {
    let [contrast, toe, shoulder, white] = KEYS.map(|key| value(settings, key));
    let p = CONTRAST * 2f64.powf(0.5 * contrast);
    let toe = if toe > 0.0 { 0.08 * toe } else { 0.03 * toe };
    let m = if shoulder > 0.0 { 1.0 + 0.25 * shoulder } else { 1.0 };
    let grey = untoe(MID, toe);
    let s = MID / (grey / m).powf(1.0 / p) - MID;
    Params { p, s, m, toe, white: (WHITE_START + 0.15 * white).clamp(0.6, 0.95), shoulder: shoulder.min(0.0) }
}

/// `_tone`: the tone scale and a deepening toe, without the flare.
#[inline]
fn tone(x: f32, prm: &Params) -> f32 {
    let mut x = maximum(x, 0.0);
    let mid = MID as f32;
    if prm.shoulder < 0.0 {
        let stops = (maximum(x, 1e-12_f64 as f32) / mid).log2();
        if stops > 0.0 {
            x = mid * (stops * ((1.0 + 0.3 * prm.shoulder) as f32)).exp2();
        }
    }
    let mut y = (prm.m as f32) * (x / (x + prm.s as f32)).powf(prm.p as f32);
    if prm.m > 1.0 {
        y = minimum(y, 1.0);
    }
    if prm.toe > 0.0 {
        let t = prm.toe as f32;
        y = (1.0 + t) * y * y / (y + t);
    }
    y
}

/// `_flare`: a lifted black lifts every channel.
#[inline]
fn flare(y: f32, prm: &Params) -> f32 {
    let f = (-prm.toe).max(0.0) as f32;
    if f != 0.0 {
        f + (1.0 - f) * y
    } else {
        y
    }
}

/// Port of luma/tone_mapping.py `curve`: display level of scene level `x` (the largest channel).
pub fn curve(x: f32, prm: &Params) -> f32 {
    flare(tone(x, prm), prm)
}

/// `_pixels` of one pixel.
#[inline]
fn pixel(p: &mut [f32], prm: &Params) {
    let rgb = [maximum(p[0], 0.0), maximum(p[1], 0.0), maximum(p[2], 0.0)];
    let peak = maximum(maximum(rgb[0], rgb[1]), rgb[2]);
    let level = tone(peak, prm);
    let ratio = level / maximum(peak, 1e-9_f64 as f32);
    let level = flare(level, prm);
    let start = prm.white as f32;
    let toward = clip((level - start) / (1.0 - start), 0.0, 1.0);
    let toward = toward * toward;
    for c in 0..3 {
        let out = flare(rgb[c] * ratio, prm);
        p[c] = out + (level - out) * toward;
    }
}

/// Port of luma/tone_mapping.py `render`: display-linear pixels (0..1) of scene-linear working-space pixels.
pub fn render(a: &Image, settings: &Settings) -> Image {
    let prm = params(settings);
    let mut out = a.as_standard_layout().into_owned();
    pixels_mut(&mut out, |p| pixel(p, &prm));
    out
}

/// Port of luma/tone_mapping.py `look_of`: the name of the look the settings match, or "" for a custom one
/// (also "" where the oracle raises on a text value).
pub fn look_of(settings: &Settings) -> &'static str {
    let mut values = [0f64; 4];
    for (slot, key) in values.iter_mut().zip(KEYS) {
        match setting(settings, key) {
            // round(x, 3): the decimal rounding of the exact binary value, read back.
            Some(v) if v.is_finite() => *slot = format!("{v:.3}").parse().unwrap_or(v),
            Some(v) => *slot = v,
            None => return "",
        }
    }
    LOOKS.iter().find(|(_, look)| *look == values).map_or("", |(name, _)| name)
}

/// Port of luma/tone_mapping.py `active`: the DRT is chosen and HDR editing is off.
pub fn active(settings: &Settings) -> bool {
    settings.value("tone_mapping").as_str() == Some("drt") && !settings.bool("hdr")
}
