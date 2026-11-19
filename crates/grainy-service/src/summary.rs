//! Summaries (`photo_service` "summaries"): get_photo's view of a catalogue row.

use serde_json::{Map, Value};

use crate::pyutil::{eq, get, number, parent, round, text, truthy};
use crate::{Error, Result, CURVES, FIELDS, LABELS};

/// Port of `photo_service.GROUPS`: settings groups listed by name under `other` when changed.
pub const GROUPS: [(&str, &[&str]); 12] = [
    (
        "crop",
        &[
            "crop",
            "upright",
            "perspective_v",
            "perspective_h",
            "aspect_scale",
            "shift_x",
            "shift_y",
            "transform_scale",
            "guides",
        ],
    ),
    ("parametric_curve", &["parametric"]),
    ("color_mix", &["hsl", "mixer_mode", "calibration", "point_colors"]),
    ("grading", &["grading", "grading_balance"]),
    ("raw_color", &["raw_mode", "raw_kelvin", "raw_tint", "raw_neutral", "kelvin", "kelvin_enabled", "wb_reference"]),
    ("profile", &["dcp_profile", "dcp_huesat", "dcp_look", "dcp_tone", "dcp_exposure", "working_space"]),
    ("detail", &["texture", "dehaze", "noise_luma", "noise_color", "sharpen_radius", "sharpen_detail", "sharpen_mask"]),
    (
        "lens",
        &[
            "distortion",
            "distortion_k2",
            "distortion_k3",
            "lens_vignette",
            "lens_profile",
            "ca_red",
            "ca_blue",
            "defringe",
            "camera_matrix",
            "lensfun",
            "lensfun_enabled",
            "lensfun_distortion",
            "lensfun_tca",
            "lensfun_vignette",
            "lensfun_scale",
        ],
    ),
    ("negative", &["negative", "negative_base", "negative_peak"]),
    ("photo_filter", &["photo_filter", "photo_filter_enabled", "photo_filter_density", "photo_filter_luminosity"]),
    ("film", &["film_look", "film_strength"]),
    ("tone_mapping", &["tone_mapping", "drt_contrast", "drt_toe", "drt_shoulder", "drt_white"]),
];
/// Port of `photo_service.COUNTED`.
pub const COUNTED: [&str; 2] = ["masks", "retouch"];
/// Port of `photo_service.IGNORED`.
pub const IGNORED: [&str; 11] = [
    "tone_version",
    "detail_version",
    "video_color",
    "video_time",
    "hdr",
    "hdr_limit",
    "hdr_brightness",
    "sdr_exposure",
    "sdr_compression",
    "upright_crop",
    "panels_off",
];
/// Port of `photo_service.INFO`: summary name to info key.
pub const INFO: [(&str, &str); 8] = [
    ("format", "format"),
    ("camera", "camera"),
    ("lens", "lens"),
    ("iso", "iso"),
    ("shutter", "shutter"),
    ("aperture", "aperture"),
    ("focal", "focal_length"),
    ("date", "date"),
];

/// Port of `photo_service.fmt`: numbers rounded to two decimals, integral ones as integers; booleans as they are.
pub fn fmt(value: &Value) -> Result<Value> {
    match value {
        Value::Bool(_) => Ok(value.clone()),
        Value::Number(n) => {
            let number = round(n.as_f64().unwrap_or(0.0), 2);
            Ok(integral(number))
        }
        _ => Err(Error::Type(format!("float() argument must be a string or a real number, not {}", type_name(value)))),
    }
}

fn integral(number: f64) -> Value {
    if number.fract() == 0.0 && number.abs() < 9.2e18 {
        Value::from(number as i64)
    } else {
        Value::from(number)
    }
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "'NoneType'",
        Value::String(_) => "'str'",
        Value::Array(_) => "'list'",
        Value::Object(_) => "'dict'",
        _ => "'float'",
    }
}

fn label_name(label: &Value) -> Value {
    LABELS.iter().find(|(_, k)| label == k).map_or_else(|| label.clone(), |(n, _)| Value::String((*n).into()))
}

fn shown(key: &str, value: &Value) -> Result<Value> {
    match key {
        "rotation" => {
            let times = match value {
                Value::Number(n) if n.is_f64() => Value::from(n.as_f64().unwrap_or(0.0) * 90.0),
                Value::Number(n) => Value::from(n.as_i64().unwrap_or(0) * 90),
                Value::Bool(b) => Value::from(i64::from(*b) * 90),
                other => other.clone(),
            };
            fmt(&times)
        }
        "film_look" => {
            let name = value.as_str().unwrap_or("");
            Ok(match grainy_film::entry(name) {
                Some(film) if value.is_string() => Value::String(film.name.clone()),
                _ => Value::String(text(value)),
            })
        }
        _ => fmt(value),
    }
}

fn point(p: &Value) -> Value {
    Value::Array(
        p.as_array()
            .map(|v| v.iter().map(|x| Value::from(round(x.as_f64().unwrap_or(0.0), 4))).collect())
            .unwrap_or_default(),
    )
}

/// Port of `photo_service.summary`: get_photo's view of a catalogue row (no GPS, serials, owners or maker notes).
pub fn summary(record: &Value) -> Result<Value> {
    let base = Value::Object(grainy_core::defaults().0);
    let settings = get(record, "settings");
    let info = match record.get("info") {
        Some(v) if truthy(v) => v.clone(),
        _ => Value::Object(Map::new()),
    };
    let mut result = Map::new();
    for key in ["id", "name", "path"] {
        result.insert(key.into(), record.get(key).cloned().ok_or_else(|| Error::Key(format!("'{key}'")))?);
    }
    if truthy(get(settings, "crop")) {
        result.insert("crop".into(), settings["crop"].clone());
    }
    if truthy(get(settings, "upright")) {
        let rotation = settings["upright"].get("rotation").ok_or_else(|| Error::Key("'rotation'".into()))?;
        result.insert("auto_level".into(), fmt(rotation)?);
    }
    if truthy(get(&info, "width")) && truthy(get(&info, "height")) {
        result.insert("size".into(), Value::Array(vec![info["width"].clone(), info["height"].clone()]));
    }
    for (name, key) in INFO {
        let value = get(&info, key);
        if value.is_null() || value == "" || eq(value, &Value::from(0)) {
            continue;
        }
        result.insert(name.into(), if number(value) { fmt(value)? } else { Value::String(text(value)) });
    }
    for (name, key) in [("r", "rating"), ("f", "flag")] {
        if truthy(get(record, key)) {
            result.insert(name.into(), record[key].clone());
        }
    }
    if truthy(get(record, "label")) {
        result.insert("l".into(), label_name(&record["label"]));
    }
    if truthy(get(record, "keywords")) {
        result.insert("keywords".into(), record["keywords"].clone());
    }
    let mut edits = Map::new();
    for (key, _, _) in FIELDS {
        if let Some(value) = settings.get(key) {
            if !eq(value, get(&base, key)) {
                edits.insert(key.into(), shown(key, value)?);
            }
        }
    }
    for key in CURVES {
        let current = settings.get(key).unwrap_or(&base[key]);
        if !eq(current, &base[key]) {
            let curves = settings.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
            let shown = if key == "curve" {
                curves.iter().map(point).collect()
            } else {
                curves
                    .iter()
                    .map(|channel| {
                        Value::Array(channel.as_array().map(|c| c.iter().map(point).collect()).unwrap_or_default())
                    })
                    .collect()
            };
            edits.insert(key.into(), Value::Array(shown));
        }
    }
    if !edits.is_empty() {
        result.insert("edits".into(), Value::Object(edits));
    }
    let mut other: Vec<Value> = GROUPS
        .iter()
        .filter(|(_, keys)| {
            !keys.iter().all(|k| FIELDS.iter().any(|(f, _, _)| f == k))
                && keys.iter().any(|k| !eq(get(settings, k), get(&base, k)))
        })
        .map(|(name, _)| Value::String((*name).into()))
        .collect();
    for key in COUNTED {
        if truthy(get(settings, key)) {
            let count = match &settings[key] {
                Value::Array(a) => a.len(),
                Value::Object(o) => o.len(),
                Value::String(s) => s.chars().count(),
                _ => 0,
            };
            other.push(Value::String(format!("{key}:{count}")));
        }
    }
    if !other.is_empty() {
        result.insert("other".into(), Value::Array(other));
    }
    if truthy(get(settings, "panels_off")) {
        result.insert("off".into(), settings["panels_off"].clone());
    }
    Ok(Value::Object(result))
}

/// Port of `photo_service.summaries`: `(dirs, entries)` with each folder once and `d` its index.
pub fn summaries(records: &[Value]) -> Result<(Vec<String>, Vec<Value>)> {
    let (mut dirs, mut entries) = (Vec::<String>::new(), Vec::new());
    for record in records {
        let Value::Object(mut entry) = summary(record)? else { continue };
        let path = text(&entry.shift_remove("path").unwrap_or(Value::Null));
        let folder = parent(&path);
        let position = match dirs.iter().position(|d| *d == folder) {
            Some(p) => p,
            None => {
                dirs.push(folder);
                dirs.len() - 1
            }
        };
        let mut compact = Map::new();
        compact.insert("id".into(), entry.shift_remove("id").unwrap_or(Value::Null));
        let name = entry.shift_remove("name").unwrap_or(Value::Null);
        compact.insert("name".into(), name.clone());
        if position != 0 {
            compact.insert("d".into(), position.into());
        }
        let file = crate::pyutil::name(&path);
        if Value::String(file.clone()) != name {
            compact.insert("file".into(), file.into());
        }
        if !get(record, "virtual_source").is_null() {
            compact.insert("v".into(), 1.into());
        }
        compact.extend(entry);
        entries.push(Value::Object(compact));
    }
    Ok((dirs, entries))
}
