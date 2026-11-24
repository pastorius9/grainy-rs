//! Validation and composition (`photo_service` "validation and composition"): what the AI connection's
//! set/add values mean, applied as the editor's sliders would apply them.

use std::collections::BTreeSet;

use grainy_core::Settings;
use serde_json::{Map, Value};

use crate::panels::{carried, changed_keys, edit_group, panels_of, switch_on};
use crate::pyutil::{eq, float_or_zero, get, int_or_zero, number, repr, text, truthy};
use crate::{normalized, object, service, Error, Result, COPY_GROUPS, CURVES, CURVE_POINTS, FIELDS, GEOMETRY};

/// One group of changes: `parse_changes`'s `(changes, operations)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Changes {
    /// `{key: float | bool | str | curve points}` in the order given (set values, then add values).
    pub values: Map<String, Value>,
    /// `{key: "set" | "add"}`.
    pub operations: Map<String, Value>,
}
impl Changes {
    fn adds(&self, key: &str) -> bool {
        self.operations.get(key).and_then(Value::as_str) == Some("add")
    }
    fn sets(&self, key: &str) -> bool {
        self.operations.get(key).and_then(Value::as_str) == Some("set")
    }
}

/// What a preset or pasted settings need fitted to the receiving photo, from modules outside this crate:
/// `adobe_preset.resolve`, `optics.retarget` and `dcp.camera_matches` (used by the ported `rawcolor.retarget`).
pub trait Fitting {
    /// `adobe_preset.resolve(preset, base, path)`; `Err(name)` names the Python exception class (only
    /// `ValueError` and `TypeError` are turned into a ServiceError by the oracle, any name is accepted here).
    fn resolve(&self, preset: &Value, settings: &Value, path: &str) -> std::result::Result<Value, String>;
    /// `optics.retarget(settings, info)`; `Err(LensMismatch)` stands for its `ValueError`.
    fn lens(&self, settings: &Value, info: &Value) -> std::result::Result<Value, LensMismatch>;
    /// `dcp.camera_matches(dcp.compiled(profile), info)`.
    fn camera_matches(&self, profile: &Value, info: &Value) -> bool;
}

/// `optics.retarget`'s `ValueError`: the lens profile does not fit the photo's lens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LensMismatch;

/// The fitting of photos without imported Adobe presets, lens profiles or DCP camera profiles, exactly as the
/// oracle does them: a Grainy preset is normalized, settings without `lensfun` are unchanged, and a camera
/// profile is taken to match. An Adobe preset or a lens profile is refused (`NotImplementedError` / the lens
/// error); production passes the importer and lens crates' implementation instead.
#[derive(Clone, Copy, Debug, Default)]
pub struct Plain;
impl Fitting for Plain {
    fn resolve(&self, preset: &Value, _settings: &Value, _path: &str) -> std::result::Result<Value, String> {
        if preset.get("_adobe_preset").is_some() {
            return Err("NotImplementedError".into());
        }
        normalized(preset).map_err(|_| "TypeError".to_owned())
    }
    fn lens(&self, settings: &Value, _info: &Value) -> std::result::Result<Value, LensMismatch> {
        if truthy(get(settings, "lensfun")) {
            Err(LensMismatch)
        } else {
            Ok(settings.clone())
        }
    }
    fn camera_matches(&self, _profile: &Value, _info: &Value) -> bool {
        true
    }
}

pub(crate) fn field(key: &str) -> Option<(i64, i64)> {
    FIELDS.iter().find(|(k, _, _)| *k == key).map(|(_, low, high)| (*low, *high))
}

/// Port of `photo_service.curve_points`: a curve as the editor stores it, `[[x, y], ...]` as floats.
pub fn curve_points(value: &Value, key: &str) -> Result<Value> {
    let message = format!("{key} must be 2 to {CURVE_POINTS} [x, y] points, x and y 0..1, x rising from 0 to 1");
    let Some(list) = value.as_array().filter(|l| (2..=CURVE_POINTS).contains(&l.len())) else {
        return Err(service(message));
    };
    let mut points = Vec::new();
    for point in list {
        let ok = point.as_array().is_some_and(|p| {
            p.len() == 2 && p.iter().all(|v| number(v) && (0.0..=1.0).contains(&v.as_f64().unwrap_or(-1.0)))
        });
        if !ok {
            return Err(service(message));
        }
        let p =
            point.as_array().map(|p| [p[0].as_f64().unwrap_or(0.0), p[1].as_f64().unwrap_or(0.0)]).unwrap_or([0.0; 2]);
        points.push(p);
    }
    let xs: Vec<f64> = points.iter().map(|p| p[0]).collect();
    if xs[0] != 0.0 || xs[xs.len() - 1] != 1.0 || xs.windows(2).any(|w| w[1] <= w[0]) {
        return Err(service(message));
    }
    Ok(Value::Array(points.into_iter().map(|[x, y]| Value::from(vec![x, y])).collect()))
}

/// Port of `photo_service.parse_changes(set_values, add_values)`. `None` stands for a missing argument.
pub fn parse_changes(set_values: Option<&Value>, add_values: Option<&Value>) -> Result<Changes> {
    let empty = Value::Object(Map::new());
    let pick = |v: Option<&Value>| match v {
        Some(v) if truthy(v) => v.clone(),
        _ => empty.clone(),
    };
    let (set_values, add_values) = (pick(set_values), pick(add_values));
    let (Some(set_map), Some(add_map)) = (set_values.as_object(), add_values.as_object()) else {
        return Err(service("set and add must be objects"));
    };
    if set_map.is_empty() && add_map.is_empty() {
        return Err(service("give at least one value in set or add"));
    }
    let mut changes = Changes::default();
    for (operation, values) in [("set", set_map), ("add", add_map)] {
        for (key, value) in values {
            if CURVES.contains(&key.as_str()) {
                if operation != "set" {
                    return Err(service(format!("{key} can only be set")));
                }
                let parsed = if key == "curve" {
                    curve_points(value, "curve")?
                } else {
                    let Some(list) = value.as_array().filter(|l| l.len() == 3) else {
                        return Err(service("rgb_curves must be 3 curves: red, green, blue"));
                    };
                    Value::Array(
                        list.iter()
                            .enumerate()
                            .map(|(n, c)| curve_points(c, &format!("rgb_curves[{n}]")))
                            .collect::<Result<_>>()?,
                    )
                };
                changes.values.insert(key.clone(), parsed);
                changes.operations.insert(key.clone(), operation.into());
                continue;
            }
            let Some((low, high)) = field(key) else {
                return Err(service(format!("{key} is not an edit value Grainy accepts")));
            };
            if changes.values.contains_key(key) {
                return Err(service(format!("{key} is given in both set and add")));
            }
            let parsed = match key.as_str() {
                "monochrome" | "flip" => {
                    if operation != "set" {
                        return Err(service(format!("{key} can only be set (true or false)")));
                    }
                    if !value.is_boolean() {
                        return Err(service(format!("{key} must be true or false")));
                    }
                    value.clone()
                }
                "rotation" => {
                    let v = value.as_f64().unwrap_or(f64::NAN);
                    if !number(value)
                        || v % 90.0 != 0.0
                        || (operation == "set" && !(0.0..=270.0).contains(&v))
                        || v.abs() > 270.0
                    {
                        return Err(service("rotation must be 0, 90, 180 or 270 (add: a multiple of 90 up to 270)"));
                    }
                    Value::from(v)
                }
                "film_look" => {
                    let Some(name) = value.as_str().filter(|_| operation == "set") else {
                        return Err(service("film_look can only be set, to a film name"));
                    };
                    match grainy_film::find(Some(name)) {
                        grainy_film::Found::Off => Value::String(String::new()),
                        grainy_film::Found::Stock(id) => Value::String(id),
                        grainy_film::Found::Suggestions(close) => {
                            let tail = if close.is_empty() {
                                "; call list_films for the film names".to_owned()
                            } else {
                                format!("; close: {}", close.join(", "))
                            };
                            return Err(service(format!("no film named {}{tail}", repr(name))));
                        }
                    }
                }
                _ => {
                    if !number(value) {
                        return Err(service(format!("{key} must be a number")));
                    }
                    let v = value.as_f64().unwrap_or(0.0);
                    if operation == "set" && !(low as f64 <= v && v <= high as f64) {
                        return Err(service(format!("{key} must be between {low} and {high}")));
                    }
                    if operation == "add" && v.abs() > (high - low) as f64 {
                        return Err(service(format!("{key} change must be between {} and {}", low - high, high - low)));
                    }
                    Value::from(v)
                }
            };
            changes.values.insert(key.clone(), parsed);
            changes.operations.insert(key.clone(), operation.into());
        }
    }
    Ok(changes)
}

/// Port of `photo_service.canvas`: oriented (width, height) of the uncropped photo (the info's own values),
/// `None` when unknown.
pub fn canvas(settings: &Value, info: &Value) -> Option<(Value, Value)> {
    let (w, h) = (get(info, "width"), get(info, "height"));
    if !truthy(w) || !truthy(h) {
        return None;
    }
    if int_or_zero(get(settings, "rotation")).rem_euclid(2) == 1 {
        Some((h.clone(), w.clone()))
    } else {
        Some((w.clone(), h.clone()))
    }
}

fn crop_of(value: &Value) -> Option<[f64; 4]> {
    let list = value.as_array().filter(|l| !l.is_empty())?;
    let v: Vec<f64> = list.iter().map(float_or_zero).collect();
    (v.len() == 4).then(|| [v[0], v[1], v[2], v[3]])
}

/// Port of `photo_service.edited`: settings with one group of changes, as the editor's sliders make them (pure).
/// `ident` is the photo id used in messages.
pub fn edited(settings: &Value, changes: &Changes, info: &Value, ident: i64) -> Result<Value> {
    let original = object(settings)?;
    if crate::NEGATIVE_ONLY.iter().any(|k| changes.values.contains_key(*k)) && !truthy(get(settings, "negative")) {
        return Err(service(format!(
            "photo {ident} is not an inverted negative; negative_density and negative_range apply only after invert_negatives"
        )));
    }
    let mut result = original.clone();
    let previous_angle = float_or_zero(get(settings, "straighten"));
    if let Some(film) = changes.values.get("film_look") {
        let explicit: Vec<&str> = changes.values.keys().map(String::as_str).collect();
        result = grainy_film::with_film(&Settings(result), film.as_str().unwrap_or(""), &explicit).0;
    }
    for (key, value) in &changes.values {
        match key.as_str() {
            "monochrome" | "flip" => {
                result.insert(key.clone(), Value::Bool(truthy(value)));
            }
            "film_look" => {}
            "curve" | "rgb_curves" => {
                result.insert(key.clone(), value.clone());
            }
            "rotation" => {
                let turns = (value.as_f64().unwrap_or(0.0) / 90.0).floor() as i64;
                let current = int_or_zero(result.get("rotation").unwrap_or(&Value::Null));
                let quarter = if changes.sets(key) { turns.rem_euclid(4) } else { (current + turns).rem_euclid(4) };
                if quarter != current {
                    result.insert("rotation".into(), quarter.into());
                    result.insert("crop".into(), Value::Null);
                }
            }
            _ => {
                let (low, high) = field(key).unwrap_or((0, 0));
                let mut v = value.as_f64().unwrap_or(0.0);
                if changes.adds(key) {
                    let sum = float_or_zero(result.get(key).unwrap_or(&Value::Null)) + v;
                    // Python max(low, min(high, sum)): the second argument only when strictly beyond.
                    let upper = if sum < high as f64 { sum } else { high as f64 };
                    v = if upper > low as f64 { upper } else { low as f64 };
                }
                result.insert(key.clone(), Value::from(v));
            }
        }
    }
    if (changes.values.contains_key("highlights") || changes.values.contains_key("shadows"))
        && result.get("tone_version").is_some_and(|v| eq(v, &Value::from(2)))
    {
        result.insert("tone_version".into(), 3.into());
    }
    if changes.values.contains_key("straighten") && !truthy(get(settings, "upright")) {
        let current = Value::Object(result.clone());
        let Some((w, h)) = canvas(&current, info).map(|(w, h)| (float_or_zero(&w), float_or_zero(&h))) else {
            return Err(service(format!("photo {ident}: its size is not known yet; open it once in Grainy")));
        };
        let angle = float_or_zero(get(&current, "straighten"));
        let crop = crate::straighten::refit(crop_of(get(&current, "crop")), previous_angle, angle, w, h);
        result.insert("crop".into(), crop.map_or(Value::Null, |c| Value::from(c.to_vec())));
    }
    let keys = changed_keys(original, &result);
    switch_on(&mut result, &keys);
    normalized(&Value::Object(result))
}

/// One step of [`compose`]: a group of changes, or a function of the settings so far (a preset, pasted settings).
pub enum Step<'a> {
    /// `edited(result, changes, ...)`.
    Edit(&'a Changes),
    /// `group(result)`.
    Apply(&'a dyn Fn(&Value) -> Result<Value>),
}

/// Port of `photo_service.compose`: each group in order, later ones refining earlier ones.
pub fn compose(settings: &Value, groups: &[Step<'_>], info: &Value, ident: i64) -> Result<Value> {
    let mut result = settings.clone();
    for group in groups {
        result = match group {
            Step::Edit(changes) => edited(&result, changes, info, ident)?,
            Step::Apply(f) => f(&result)?,
        };
    }
    Ok(result)
}

/// Port of `rawcolor.retarget`: RAW colour settings fitted to a photo (camera matching injected).
pub fn raw_retarget(settings: &Value, info: &Value, fitting: &dyn Fitting) -> Result<Value> {
    let mut result = object(settings)?.clone();
    let format = match info.get("format") {
        Some(v) => text(v),
        None => String::new(),
    };
    let is_raw = crate::RAW_EXTENSIONS.contains(&format!(".{}", format.to_lowercase()).as_str());
    if !is_raw {
        result.insert("raw_mode".into(), "legacy".into());
        result.insert("raw_neutral".into(), Value::Null);
        result.insert("dcp_profile".into(), Value::Null);
    } else if let Some(profile) = result.get("dcp_profile").filter(|v| truthy(v)) {
        if !fitting.camera_matches(profile, info) {
            result.insert("dcp_profile".into(), Value::Null);
        }
    }
    if result.get("raw_mode").and_then(Value::as_str) == Some("sample") {
        result.insert("raw_mode".into(), "custom".into());
        result.insert("raw_neutral".into(), Value::Null);
    }
    Ok(Value::Object(result))
}

/// Port of `photo_service.preset_settings`: settings after applying a preset as the Presets panel does. `record`
/// needs `path` and `info`.
pub fn preset_settings(preset: &Value, settings: &Value, record: &Value, fitting: &dyn Fitting) -> Result<Value> {
    let path = crate::text_of(record, "path")?;
    let info = get(record, "info");
    let value = fitting
        .resolve(preset, settings, path)
        .map_err(|kind| service(format!("the preset cannot be applied to this photo ({kind})")))?;
    let value = fitting
        .lens(&value, info)
        .map_err(|LensMismatch| service("the preset's lens profile does not fit this photo's lens"))?;
    let mut value = object(&raw_retarget(&value, info, fitting)?)?.clone();
    for key in GEOMETRY {
        let kept = settings.get(key).ok_or_else(|| Error::Key(format!("'{key}'")))?;
        value.insert(key.into(), kept.clone());
    }
    normalized(&Value::Object(value))
}

/// Port of `photo_service.copy_keys`: the settings `copy_from` copies, Grainy's default copy items or the named
/// `COPY_GROUPS`.
pub fn copy_keys(groups: Option<&[String]>) -> Result<BTreeSet<String>> {
    match groups {
        Some(names) if !names.is_empty() => {
            let mut keys = BTreeSet::new();
            for name in names {
                let group = COPY_GROUPS
                    .iter()
                    .find(|(k, _)| k == name)
                    .and_then(|(_, g)| edit_group(g))
                    .ok_or_else(|| Error::Key(repr(name)))?;
                keys.extend(group.iter().map(|k| (*k).to_owned()));
            }
            Ok(keys)
        }
        _ => {
            let crop = edit_group("크롭·회전").unwrap_or(&[]);
            Ok(grainy_catalog::settings(&Value::Object(Map::new()))?
                .0
                .keys()
                .filter(|k| !crop.contains(&k.as_str()) && *k != "panels_off")
                .cloned()
                .chain(std::iter::once("ai_denoise".to_owned()))
                .collect())
        }
    }
}

/// Port of `photo_service.copied_settings`: settings with `keys` pasted from `source`, as Paste Settings does.
pub fn copied_settings(
    source: &Value,
    settings: &Value,
    info: &Value,
    keys: &BTreeSet<String>,
    fitting: &dyn Fitting,
) -> Result<Value> {
    let (source_map, target) = (object(source)?, object(settings)?);
    let mut value = target.clone();
    for key in keys {
        if let Some(v) = source_map.get(key) {
            value.insert(key.clone(), v.clone());
        } else if key == "ai_denoise" {
            value.remove(key);
        }
    }
    let panels = panels_of(keys.iter().map(String::as_str));
    value.insert("panels_off".into(), Value::Array(carried(target, source_map, &panels)));
    let mut value = Value::Object(value);
    if keys.contains("lensfun") {
        value = fitting.lens(&value, info).map_err(|LensMismatch| {
            service("the source's lens profile does not fit this photo's lens; copy without the lens group")
        })?;
    }
    normalized(&raw_retarget(&value, info, fitting)?)
}
