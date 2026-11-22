//! The develop settings: the oracle's dict, kept as a JSON map with the oracle's keys and defaults.
//!
//! Ports `DEFAULTS`, `defaults()`, `normalized()` and `tone_white()` of `luma/engine.py`, `white_balance()` and
//! `kelvin_rgb()` of `luma/processing.py`, and `effective()` of `luma/panel_keys.py`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use serde_json::{Map, Value};

use crate::{Error, Result};

const DEFAULTS_JSON: &str = include_str!("../assets/defaults.json");
const PANELS_JSON: &str = include_str!("../assets/panels.json");

/// The HDR editing mode is switched off in the shipped oracle (`engine.HDR_FEATURE = False`). This is the value
/// every process starts with; [`set_hdr_feature`] switches it for the whole process (the oracle's tests and
/// self-test assign `engine.HDR_FEATURE = True` the same way).
pub const HDR_FEATURE: bool = false;

static HDR_ENABLED: AtomicBool = AtomicBool::new(HDR_FEATURE);

/// Port of reading `engine.HDR_FEATURE`: whether the HDR editing mode is on in this process (`normalized` keeps
/// `hdr` only then).
pub fn hdr_feature() -> bool {
    HDR_ENABLED.load(Ordering::Relaxed)
}

/// Port of assigning `engine.HDR_FEATURE`: switches the HDR editing mode on or off for the whole process.
pub fn set_hdr_feature(enabled: bool) {
    HDR_ENABLED.store(enabled, Ordering::Relaxed);
}

fn default_map() -> &'static Map<String, Value> {
    static MAP: OnceLock<Map<String, Value>> = OnceLock::new();
    MAP.get_or_init(|| match serde_json::from_str(DEFAULTS_JSON) {
        Ok(Value::Object(map)) => map,
        _ => unreachable!("assets/defaults.json is a JSON object"),
    })
}

/// The panel table of `luma/panel_keys.py`: which settings each panel of the Develop column owns.
pub struct Panels {
    /// `PANEL_KEYS`: panel name and its setting keys.
    pub keys: Vec<(String, Vec<String>)>,
    /// `OFF_PANELS`: the panels that can be switched off, in the oracle's order.
    pub off_panels: Vec<String>,
    /// `ALWAYS`: keys no panel switch affects.
    pub always: Vec<String>,
    /// `SWITCHES`: panels switched by a setting of their own (panel name, boolean setting key).
    pub switches: Vec<(String, String)>,
}

impl Panels {
    /// The setting keys of a panel.
    pub fn keys_of(&self, panel: &str) -> Option<&[String]> {
        self.keys.iter().find(|(name, _)| name == panel).map(|(_, keys)| keys.as_slice())
    }
}

/// The panel table (`panel_keys.PANEL_KEYS`, `OFF_PANELS`, `ALWAYS`, `SWITCHES`).
pub fn panels() -> &'static Panels {
    static PANELS: OnceLock<Panels> = OnceLock::new();
    PANELS.get_or_init(|| {
        let value: Value = serde_json::from_str(PANELS_JSON).expect("assets/panels.json is valid JSON");
        let names = |v: &Value| -> Vec<String> {
            v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default()
        };
        let keys = value["panel_keys"]
            .as_object()
            .map(|m| m.iter().map(|(panel, list)| (panel.clone(), names(list))).collect())
            .unwrap_or_default();
        let switches = value["switches"]
            .as_object()
            .map(|m| m.iter().filter_map(|(panel, key)| Some((panel.clone(), key.as_str()?.to_owned()))).collect())
            .unwrap_or_default();
        Panels { keys, off_panels: names(&value["off_panels"]), always: names(&value["always"]), switches }
    })
}

/// Develop settings: a JSON map with the oracle's keys. Values are read with the typed accessors; a key that is
/// missing reads as its default, as `dict.get(key, DEFAULTS[key])` would.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings(pub Map<String, Value>);

/// Port of `engine.defaults()`: a fresh copy of the default settings.
pub fn defaults() -> Settings {
    Settings(default_map().clone())
}

/// Python truthiness of a JSON value (`bool(value)`).
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

impl Settings {
    /// Settings from a stored JSON object, as it is (not normalized).
    pub fn from_value(value: Value) -> Result<Self> {
        match value {
            Value::Object(map) => Ok(Self(map)),
            _ => Err(Error::NotAnObject),
        }
    }

    /// The value of `key`, or its default when missing (`Null` for a key the oracle does not know).
    pub fn value(&self, key: &str) -> &Value {
        static NULL: Value = Value::Null;
        self.0.get(key).or_else(|| default_map().get(key)).unwrap_or(&NULL)
    }

    /// A number as `f64` (Python's float). Booleans read as 0/1, anything else as 0.
    pub fn f64(&self, key: &str) -> f64 {
        match self.value(key) {
            Value::Number(n) => n.as_f64().unwrap_or(0.0),
            Value::Bool(b) => f64::from(u8::from(*b)),
            _ => 0.0,
        }
    }

    /// A number as `f32`.
    pub fn f32(&self, key: &str) -> f32 {
        self.f64(key) as f32
    }

    /// A number as `i64` (truncated).
    pub fn i64(&self, key: &str) -> i64 {
        match self.value(key) {
            Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().unwrap_or(0.0) as i64),
            Value::Bool(b) => i64::from(*b),
            _ => 0,
        }
    }

    /// Python truthiness of the value (`if s['key']:`).
    pub fn bool(&self, key: &str) -> bool {
        truthy(self.value(key))
    }

    /// A string value, or `""` when the value is not a string.
    pub fn str(&self, key: &str) -> &str {
        self.value(key).as_str().unwrap_or("")
    }

    /// Whether the value is `None`.
    pub fn is_none(&self, key: &str) -> bool {
        self.value(key).is_null()
    }

    /// Sets a value.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        self.0.insert(key.to_owned(), value.into());
    }
}

/// Port of `processing.kelvin_rgb`: relative RGB of a Planckian-locus approximation at `kelvin`
/// (float64 maths, float32 result, never below 0.02).
pub fn kelvin_rgb(kelvin: f64) -> [f32; 3] {
    let t = kelvin.clamp(2000.0, 50000.0) / 100.0;
    let red = if t <= 66.0 { 255.0 } else { 329.698727446 * (t - 60.0).powf(-0.1332047592) };
    let green = if t <= 66.0 {
        99.4708025861 * t.ln() - 161.1195681661
    } else {
        288.1221695283 * (t - 60.0).powf(-0.0755148492)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.5177312231 * (t - 10.0).ln() - 305.0447927307
    };
    let channel = |x: f64| (x.clamp(0.0, 255.0) / 255.0).max(0.02) as f32;
    [channel(red), channel(green), channel(blue)]
}

/// Port of `processing.white_balance`: the per-channel gain of temperature, tint and the Kelvin conversion.
pub fn white_balance(s: &Settings) -> [f32; 3] {
    let temperature = s.f64("temperature");
    let tint = s.f64("tint");
    let mut gain = [
        2f64.powf(temperature / 180.0 + tint / 400.0) as f32,
        2f64.powf(-tint / 250.0) as f32,
        2f64.powf(-temperature / 180.0 + tint / 400.0) as f32,
    ];
    if s.bool("kelvin_enabled") {
        let reference = kelvin_rgb(s.f64("wb_reference"));
        let target = kelvin_rgb(s.f64("kelvin"));
        for c in 0..3 {
            gain[c] *= reference[c] / target[c];
        }
        let green = gain[1];
        for value in &mut gain {
            *value /= green;
        }
    }
    gain
}

/// Port of `engine.tone_white`: the largest linear gain the tone stage applies before encoding.
pub fn tone_white(s: &Settings) -> f64 {
    let lift = (1.0 + 2.0 * s.f64("lens_vignette") / 100.0).max(1.0);
    let gain = white_balance(s);
    let largest = gain.iter().copied().fold(f32::MIN, f32::max);
    2f64.powf(s.f64("exposure")) * f64::from(largest) * lift
}

/// Port of `engine.normalized`: stored settings completed with defaults, unknown keys dropped, and the process
/// versions of edits saved before those versions existed.
pub fn normalized(stored: &Map<String, Value>) -> Settings {
    let known = default_map();
    let mut result = defaults();
    for (key, value) in stored {
        if known.contains_key(key) {
            result.0.insert(key.clone(), value.clone());
        } else if key == "ai_denoise" {
            if let Some(v) = value.as_f64().filter(|v| v.is_finite() && *v > 0.0) {
                result.0.insert(key.clone(), Value::from(v.clamp(0.0, 100.0)));
            }
        }
    }
    if !hdr_feature() {
        result.set("hdr", false);
    }
    if !stored.contains_key("tone_version") {
        let version = if tone_white(&result) > 1.0 && !result.bool("hdr") { 1 } else { 2 };
        result.set("tone_version", version);
    }
    if result.i64("tone_version") == 2 && !result.bool("shadows") && !result.bool("highlights") {
        result.set("tone_version", 3);
    }
    if !stored.contains_key("detail_version") {
        let version = if result.bool("sharpen") {
            1
        } else if result.bool("noise_luma") {
            2
        } else {
            3
        };
        result.set("detail_version", version);
    }
    if result.bool("panels_off") {
        let ordered: Vec<Value> = match result.value("panels_off") {
            Value::Array(off) => panels()
                .off_panels
                .iter()
                .filter(|panel| off.iter().any(|x| x.as_str() == Some(panel.as_str())))
                .map(|panel| Value::String(panel.clone()))
                .collect(),
            _ => Vec::new(),
        };
        result.set("panels_off", ordered);
    }
    result
}

/// Port of `panel_keys.effective`: the settings as rendered, with the values of switched-off panels replaced by
/// their defaults.
pub fn effective(settings: &Settings) -> Settings {
    let off: Vec<String> = match settings.value("panels_off") {
        Value::Array(list) => list.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    };
    if off.is_empty() {
        return settings.clone();
    }
    let mut result = settings.clone();
    let known = default_map();
    for panel in &off {
        if let Some((_, keys)) = panels().keys.iter().find(|(name, _)| name == panel) {
            for key in keys {
                if let Some(value) = known.get(key) {
                    result.0.insert(key.clone(), value.clone());
                }
            }
        }
        if panel == "detail" {
            result.0.remove("ai_denoise");
        }
    }
    result
}

/// Python `==` on JSON values: 0, 0.0 and False are equal. That controls edit no-ops.
pub fn equivalent(a: &Value, b: &Value) -> bool {
    fn integer(v: &Value) -> Option<i128> {
        match v {
            Value::Bool(b) => Some(i128::from(*b)),
            Value::Number(n) => n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from)),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Array(a), Value::Array(b)) => a.len() == b.len() && a.iter().zip(b).all(|(a, b)| equivalent(a, b)),
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len() && a.iter().all(|(k, a)| b.get(k).is_some_and(|b| equivalent(a, b)))
        }
        (Value::Number(_) | Value::Bool(_), Value::Number(_) | Value::Bool(_)) => match (integer(a), integer(b)) {
            (Some(a), Some(b)) => a == b,
            (Some(i), None) | (None, Some(i)) => {
                let f = if integer(a).is_none() { a.as_f64() } else { b.as_f64() };
                f.is_some_and(|f| f.fract() == 0.0 && f as i128 == i)
            }
            (None, None) => a.as_f64() == b.as_f64(),
        },
        _ => a == b,
    }
}
