//! Port of `luma/photo_edits.py`: compose an AI batch off the GUI thread, in entry order, without catalogue writes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use crate::auto::reason;
use crate::edits::{copied_settings, edited, preset_settings, Changes, Fitting};
use crate::pyutil::{get, number};
use crate::{id_of, normalized, service, text_of, Error, Result};

/// Port of `photo_edits.crop_value`: `None` or `[left, top, right, bottom]` in 0..1 as floats.
pub fn crop_value(value: &Value) -> Result<Value> {
    if value.is_null() {
        return Ok(Value::Null);
    }
    let ok = value.as_array().is_some_and(|v| {
        v.len() == 4 && v.iter().all(|x| number(x) && (0.0..=1.0).contains(&x.as_f64().unwrap_or(-1.0)))
    });
    if !ok {
        return Err(service("crop must be null or [left, top, right, bottom] in 0..1"));
    }
    let v: Vec<f64> = value.as_array().map(|a| a.iter().filter_map(Value::as_f64).collect()).unwrap_or_default();
    if v[0] >= v[2] || v[1] >= v[3] {
        return Err(service("crop needs left < right and top < bottom"));
    }
    Ok(json!(v))
}

/// A preset entry: `entry['preset_name']` and `entry['base']` (the preset's settings).
#[derive(Clone, Debug)]
pub struct PresetChoice {
    /// The preset's name as found.
    pub name: String,
    /// The preset's settings.
    pub base: Value,
}

/// One parsed `apply_edits` entry as `photo_edits.compose` reads it.
#[derive(Clone, Debug, Default)]
pub struct EditEntry {
    /// The photos of the entry.
    pub ids: Vec<i64>,
    /// `copy_from`.
    pub copy_from: Option<i64>,
    /// The keys `copy_from` copies (`copy_keys`).
    pub keys: BTreeSet<String>,
    /// `preset` (with its resolved name and settings).
    pub preset: Option<PresetChoice>,
    /// `changes` (from set/add).
    pub changes: Option<Changes>,
    /// `auto_straighten`.
    pub auto_straighten: bool,
    /// `auto_crop`.
    pub auto_crop: bool,
    /// `crop` when the entry has the key (`Some(Null)` for crop: null).
    pub crop: Option<Value>,
}

/// `(used records, their settings, failures, presets)` of [`compose`].
pub type Composed = (Vec<Value>, Vec<Value>, Vec<Value>, BTreeMap<i64, String>);

/// Port of `photo_edits.compose`. `check_deadline` is `mcp_io.check_deadline` (its error ends the call);
/// `automatic` is `photo_edits.automatic` (the crop tool's auto level and the scan-border crop on the photo's
/// own pixels): `automatic(record, settings, auto_straighten, auto_crop)`.
pub fn compose(
    entries: &[EditEntry],
    records: &[Value],
    sources: &BTreeMap<i64, Value>,
    fitting: &dyn Fitting,
    check_deadline: &mut dyn FnMut() -> Result<()>,
    automatic: &mut dyn FnMut(&Value, &Value, bool, bool) -> Result<Value>,
) -> Result<Composed> {
    let mut originals: BTreeMap<i64, &Value> = BTreeMap::new();
    for r in records {
        originals.insert(id_of(r)?, r);
    }
    let mut states: BTreeMap<i64, Value> = BTreeMap::new();
    for (ident, r) in sources {
        states.insert(*ident, get(r, "settings").clone());
    }
    for (ident, r) in &originals {
        states.insert(*ident, get(r, "settings").clone());
    }
    let mut failures: Vec<(i64, String)> = Vec::new();
    let mut presets: BTreeMap<i64, String> = BTreeMap::new();
    for entry in entries {
        check_deadline()?;
        let source = match entry.copy_from {
            Some(from) => Some(states.get(&from).cloned().ok_or_else(|| Error::Key(from.to_string()))?),
            None => None,
        };
        for ident in &entry.ids {
            if !originals.contains_key(ident) || failures.iter().any(|(i, _)| i == ident) {
                continue;
            }
            check_deadline()?;
            let record = originals[ident];
            let mut step = || -> Result<Value> {
                let mut value = states[ident].clone();
                let info = get(record, "info");
                if let Some(preset) = &entry.preset {
                    value = preset_settings(&preset.base, &value, record, fitting)?;
                    presets.insert(*ident, preset.name.clone());
                } else if let Some(source) = &source {
                    if entry.copy_from.is_some_and(|f| failures.iter().any(|(i, _)| *i == f)) {
                        return Err(service("copy_from source could not be edited"));
                    }
                    value = copied_settings(source, &value, info, &entry.keys, fitting)?;
                }
                if let Some(changes) = &entry.changes {
                    value = edited(&value, changes, info, *ident)?;
                }
                if entry.auto_straighten || entry.auto_crop {
                    value = automatic(record, &value, entry.auto_straighten, entry.auto_crop)?;
                }
                if let Some(crop) = &entry.crop {
                    let mut map: Map<String, Value> = crate::object(&value)?.clone();
                    map.insert("crop".into(), crop.clone());
                    value = Value::Object(map);
                }
                normalized(&value)
            };
            match step() {
                Ok(value) => {
                    states.insert(*ident, value);
                }
                Err(error) => {
                    let message = match &error {
                        Error::Service(m) => m.clone(),
                        other => reason(other, text_of(record, "path").unwrap_or("")),
                    };
                    failures.push((*ident, message));
                    states.insert(*ident, get(record, "settings").clone());
                    presets.remove(ident);
                }
            }
        }
    }
    check_deadline()?;
    let used: Vec<Value> =
        records.iter().filter(|r| id_of(r).is_ok_and(|i| !failures.iter().any(|(f, _)| *f == i))).cloned().collect();
    let values = used.iter().map(|r| id_of(r).map(|i| states[&i].clone())).collect::<Result<Vec<_>>>()?;
    let failures = failures.into_iter().map(|(i, e)| json!({"id": i, "error": e})).collect();
    Ok((used, values, failures, presets))
}
