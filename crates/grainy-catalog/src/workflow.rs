//! Catalog-only storage from `luma/workflow_data.py` and `command_batch.py`.
use crate::{integer, invalid, json, object, profiles, query::fold, settings, string, Catalog, Result};
use grainy_core::Settings;
use rusqlite::params;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Port of `workflow_data.edit_signature`; Python canonical JSON and SHA-256.
pub fn edit_signature(value: &Settings) -> Result<String> {
    Ok(profiles::hash(json::dumps(&settings(&Value::Object(value.0.clone()))?.0.into(), true, true, true)))
}
/// Port of `command_batch.text_hash`; the original stored text is never normalized first.
pub fn text_hash(text: &str) -> String {
    profiles::hash(text)
}
/// Port of `workflow_data.arrange_stacks`; duplicate saved entries follow the oracle unchanged.
pub fn arrange_stacks(rows: &[Value], orders: &Value, collapsed: bool) -> Result<Vec<i64>> {
    let mut groups: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for r in rows {
        if let Some(k) = r["stack_id"].as_i64() {
            groups.entry(k).or_default().push(integer(&r["id"])?);
        }
    }
    let mut emitted = BTreeSet::new();
    let mut result = Vec::new();
    for r in rows {
        let id = integer(&r["id"])?;
        let Some(k) = r["stack_id"].as_i64() else {
            result.push(id);
            continue;
        };
        let saved = orders[k.to_string()].as_array().cloned().unwrap_or_default();
        if !collapsed && saved.is_empty() {
            result.push(id);
            continue;
        }
        if !emitted.insert(k) {
            continue;
        }
        let members = groups.get(&k).ok_or_else(|| invalid("stack group missing"))?;
        let mut ordered = saved.iter().filter_map(Value::as_i64).filter(|i| members.contains(i)).collect::<Vec<_>>();
        let seen = ordered.iter().copied().collect::<BTreeSet<_>>();
        ordered.extend(members.iter().filter(|i| !seen.contains(i)));
        result.extend(ordered.into_iter().take(if collapsed { 1 } else { usize::MAX }));
    }
    Ok(result)
}
/// Port of `workflow_data.stack_members`.
pub fn stack_members(c: &Catalog, id: i64) -> Result<Vec<i64>> {
    let Some(p) = c.photo(id)? else { return Ok(vec![]) };
    if p["stack_id"].is_null() {
        return Ok(vec![]);
    }
    c.rows("SELECT id FROM photos WHERE stack_id=? ORDER BY id", [integer(&p["stack_id"])?])?
        .iter()
        .map(|r| integer(&r["id"]))
        .collect()
}
/// Port of `workflow_data.stack_order`; position changes persist under `stack_orders`.
pub fn stack_order(c: &Catalog, id: i64, position: Option<i64>) -> Result<Vec<i64>> {
    let members = stack_members(c, id)?;
    if members.is_empty() {
        return Ok(members);
    }
    let p = c.photo(id)?.ok_or_else(|| invalid("photo missing"))?;
    let key = integer(&p["stack_id"])?.to_string();
    let mut orders = c.preference("stack_orders", json!({}))?;
    let mut ordered = orders[&key]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_i64)
        .filter(|i| members.contains(i))
        .collect::<Vec<_>>();
    let saved = ordered.clone();
    ordered.extend(members.iter().filter(|i| !saved.contains(i)));
    if let Some(position) = position {
        if let Some(i) = ordered.iter().position(|i| *i == id) {
            ordered.remove(i);
        }
        ordered.insert(position.max(0).min(ordered.len() as i64) as usize, id);
        object(&orders)?;
        orders[key] = json!(ordered);
        c.save_preference("stack_orders", &orders)?;
    }
    Ok(ordered)
}
/// Port of `workflow_data.quick_collection`; UI localization supplies the collection label.
pub fn quick_collection(c: &Catalog, label: &str) -> Result<i64> {
    let id = c.preference("quick_collection", Value::Null)?;
    if let Some(id) = id.as_i64() {
        if c.collections()?.iter().any(|r| r["id"] == id && r["rules"].is_null()) {
            return Ok(id);
        }
    }
    let id = c.add_collection(label, None, None)?;
    c.save_preference("quick_collection", &json!(id))?;
    Ok(id)
}
/// Port of `workflow_data.toggle_quick`.
pub fn toggle_quick(c: &Catalog, ids: &[i64], label: &str) -> Result<(i64, bool)> {
    let id = quick_collection(c, label)?;
    let members = c.collection_ids(id)?;
    let remove = !ids.is_empty() && ids.iter().all(|i| members.contains(i));
    if remove {
        c.transaction(|| {
            for photo in ids {
                c.db.execute(
                    "DELETE FROM collection_members WHERE collection_id=? AND photo_id=?",
                    params![id, photo],
                )?;
            }
            Ok(())
        })?;
    } else {
        c.collection_add(id, ids)?;
    }
    Ok((id, !remove))
}
const METADATA_KEYS: [&str; 11] = [
    "title",
    "caption",
    "creator",
    "copyright",
    "location",
    "city",
    "country",
    "latitude",
    "longitude",
    "DateTimeOriginal",
    "keywords",
];
fn validate_keys(v: &Value) -> Result<()> {
    if object(v)?.keys().any(|k| !METADATA_KEYS.contains(&k.as_str())) {
        return Err(invalid("unsupported metadata key"));
    }
    Ok(())
}
/// Port of `workflow_data.metadata_presets`.
pub fn metadata_presets(c: &Catalog) -> Result<Value> {
    c.preference("metadata_presets", json!({}))
}
/// Port of `workflow_data.save_metadata_preset`; validates names, GPS ranges and capture dates.
pub fn save_metadata_preset(c: &Catalog, name: &str, values: &Value) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Err(invalid("preset name is empty"));
    }
    validate_keys(values)?;
    for (k, max) in [("latitude", 90.0), ("longitude", 180.0)] {
        let v = &values[k];
        if !v.is_null() && v != "" {
            let n = v
                .as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
                .ok_or_else(|| invalid("invalid GPS coordinate"))?;
            if !n.is_finite() || n.abs() > max {
                return Err(invalid("GPS coordinate out of range"));
            }
        }
    }
    if grainy_core::settings::truthy(&values["DateTimeOriginal"]) {
        chrono::NaiveDateTime::parse_from_str(string(&values["DateTimeOriginal"])?, "%Y:%m:%d %H:%M:%S")
            .map_err(|_| invalid("invalid capture date"))?;
    }
    let mut presets = metadata_presets(c)?;
    object(&presets)?;
    presets[name] = values.clone();
    c.save_preference("metadata_presets", &presets)
}
/// Port of `workflow_data.apply_metadata_preset`; the entire batch is transactional.
pub fn apply_metadata_preset(c: &Catalog, ids: &[i64], values: &Value) -> Result<()> {
    validate_keys(values)?;
    c.transaction(|| {
        for id in ids {
            let p = c.photo(*id)?.ok_or_else(|| invalid("photo list changed"))?;
            let mut extras = object(&p["user_metadata"])?.clone();
            extras
                .extend(object(values)?.iter().filter(|(k, _)| *k != "keywords").map(|(k, v)| (k.clone(), v.clone())));
            c.db.execute(
                "UPDATE photos SET extras=?,keywords=? WHERE id=?",
                params![
                    json::dumps(&extras.into(), false, false, false),
                    string(values.get("keywords").unwrap_or(&p["keywords"]))?,
                    id
                ],
            )?;
        }
        Ok(())
    })
}
/// Port of `workflow_data.add_keyword`; Unicode casefold prevents duplicate keywords.
pub fn add_keyword(c: &Catalog, ids: &[i64], keyword: &str) -> Result<()> {
    let keyword = keyword.trim();
    if keyword.is_empty() || keyword.contains(',') {
        return Err(invalid("enter one keyword"));
    }
    c.transaction(|| {
        for id in ids {
            let p = c.photo(*id)?.ok_or_else(|| invalid("photo missing"))?;
            let mut words = string(&p["keywords"])?
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            if !words.iter().any(|w| fold(w) == fold(keyword)) {
                words.push(keyword.into());
            }
            c.db.execute("UPDATE photos SET keywords=? WHERE id=?", params![words.join(", "), id])?;
        }
        Ok(())
    })
}
/// Port of `workflow_data.record_exports`; keeps the most recent 2000 records.
pub fn record_exports(c: &Catalog, outputs: &[Value]) -> Result<()> {
    let history = c.preference("export_history", json!([]))?;
    let mut history = history.as_array().ok_or_else(|| invalid("invalid export history"))?.clone();
    let stamp = c.now();
    for item in outputs {
        let mut item = object(item)?.clone();
        item.insert("created".into(), stamp.clone().into());
        history.push(item.into());
    }
    let drop = history.len().saturating_sub(2000);
    history.drain(..drop);
    c.save_preference("export_history", &history.into())
}
/// Port of `workflow_data.export_history`; compares canonical edit signatures live.
pub fn export_history(c: &Catalog, ids: &[i64]) -> Result<Vec<Value>> {
    let history = c.preference("export_history", json!([]))?;
    let mut result = Vec::new();
    for item in history.as_array().ok_or_else(|| invalid("invalid export history"))?.iter().rev() {
        let Some(id) = item["photo_id"].as_i64() else {
            continue;
        };
        if !ids.contains(&id) {
            continue;
        }
        if let Some(p) = c.photo(id)? {
            let mut item = item.clone();
            object(&item)?;
            item["edited_since"] = (item["edit_signature"] != edit_signature(&settings(&p["settings"])?)?).into();
            result.push(item);
        }
    }
    Ok(result)
}
