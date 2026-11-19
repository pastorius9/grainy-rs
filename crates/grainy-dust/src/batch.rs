//! Data-only ports of luma/dust_batch.py. No review application or undo commands.
use crate::{invalid, Result};
use grainy_catalog::{preview, Catalog};
use serde_json::{json, Value};
use std::{collections::HashSet, path::Path};
fn video(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| ["mp4", "mov", "avi", "mkv", "m4v"].contains(&s.to_lowercase().as_str()))
}
/// Port of luma/dust_batch.py capture_scope_ids; reads lightweight rows without decoding all edits.
pub fn capture_scope_ids(
    catalog: &Catalog,
    current_id: Option<i64>,
    selected_ids: &[i64],
    folder: Option<&str>,
) -> Result<Value> {
    let current = if let Some(id) = current_id.filter(|id| *id != 0) {
        catalog.rows("SELECT path FROM photos WHERE id=?", [id])?.into_iter().next()
    } else {
        None
    };
    let folder = folder.filter(|s| !s.is_empty()).map(str::to_owned).or_else(|| {
        current.as_ref().and_then(|r| r["path"].as_str()).map(|p| {
            Path::new(p).parent().map_or_else(
                || ".".into(),
                |p| if p.as_os_str().is_empty() { ".".into() } else { p.to_string_lossy().into_owned() },
            )
        })
    });
    let mut ids = Vec::new();
    for row in catalog.rows("SELECT id,path FROM photos ORDER BY id", [])? {
        let path = row["path"].as_str().ok_or_else(|| invalid("catalog path"))?;
        if folder.as_ref().is_some_and(|f| catalog.platform().in_folder(path, f, false)) && !video(path) {
            ids.push(row["id"].clone());
        }
    }
    let mut seen = HashSet::new();
    let selected: Vec<_> = selected_ids.iter().filter(|id| seen.insert(**id)).copied().collect();
    Ok(json!({"folder":folder,"scopes":{"selection":selected,"folder":ids}}))
}
/// Port of luma/dust_batch.py capture_record; returns a deep settings snapshot and source signature.
pub fn capture_record(catalog: &Catalog, id: i64) -> Result<Value> {
    let row = catalog.photo(id)?.ok_or_else(|| invalid("photo missing from catalog"))?;
    let path = row["path"].as_str().ok_or_else(|| invalid("catalog path"))?;
    if video(path) {
        return Err(invalid("dust requires a photo, not video"));
    }
    Ok(json!({"id":id,"path":path,"settings":row["settings"],"fingerprint":preview::signature(path)}))
}
/// Port of luma/dust_batch.py verify_source and the read-only command_batch.verify guard.
pub fn verify_source(catalog: &Catalog, record: &Value) -> Result<()> {
    let id = record.get("id").and_then(Value::as_i64).ok_or_else(|| invalid("record ID"))?;
    let path = record.get("path").and_then(Value::as_str).ok_or_else(|| invalid("record path"))?;
    let now = catalog.photo(id)?.ok_or_else(|| invalid("source record changed"))?;
    if now["path"] != record["path"] || !grainy_catalog::json::equivalent(&now["settings"], &record["settings"]) {
        return Err(invalid("source record changed"));
    }
    if record.get("fingerprint").is_none_or(Value::is_null)
        || !preview::signature(path).is_some_and(|s| grainy_catalog::json::equivalent(&s, &record["fingerprint"]))
    {
        return Err(invalid("source file missing or changed"));
    }
    Ok(())
}
