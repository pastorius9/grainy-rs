//! The AI connection's batch stack (`photo_service` "batch stack", on `command_batch`): an index of the last
//! [`MCP_DEPTH`](crate::MCP_DEPTH) batches that undo and redo walk in order.

use std::collections::{BTreeMap, HashSet};

use grainy_catalog::{rusqlite, Catalog};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::batch::{self, in_transaction, ApplyOptions};
use crate::pyutil::{dumps, eq, get, text, truthy};
use crate::{id_of, profiles, service, Conflict, Error, Result, BATCH_PREFIX, INDEX_KEY, MCP_DEPTH, PRESET_KEY};

/// A source of batch ids: `secrets.token_hex(3)` (six lowercase hex digits). Inject a fixed sequence in tests.
pub type Tokens<'a> = &'a mut dyn FnMut() -> Result<String>;

/// Production token source: three random bytes from SQLite's `randomblob` (a CSPRNG), as six hex digits.
pub fn random_token(catalog: &Catalog) -> Result<String> {
    Ok(catalog.connection().query_row("SELECT lower(hex(randomblob(3)))", [], |r| r.get(0))?)
}

/// Port of `photo_service.batch_index`: the index entries, oldest first.
pub fn batch_index(catalog: &Catalog) -> Result<Vec<Value>> {
    let value = catalog.preference(INDEX_KEY, json!([]))?;
    Ok(value.as_array().map(|a| a.iter().filter(|e| e.is_object()).cloned().collect()).unwrap_or_default())
}

/// Port of `photo_service.last_batch`: the most recent applied batch.
pub fn last_batch(catalog: &Catalog) -> Result<Option<Value>> {
    Ok(batch_index(catalog)?.into_iter().rev().find(|e| get(e, "state") == "applied"))
}

/// `photo_service._write_index`, inside the batch transaction.
fn write_index(catalog: &Catalog, entries: &[Value]) -> Result<()> {
    catalog.connection().execute(
        "INSERT OR REPLACE INTO preferences VALUES(?,?)",
        params![INDEX_KEY, dumps(&Value::Array(entries.to_vec()), false)],
    )?;
    Ok(())
}

/// `photo_service._set_preset`, inside the batch transaction: set (or with a falsy name remove) the photo's
/// `_preset`; returns the value before (`Null` when there was none).
fn set_preset(catalog: &Catalog, ident: i64, name: &Value) -> Result<Value> {
    let db = catalog.connection();
    let row: Option<Option<String>> =
        db.query_row("SELECT extras FROM photos WHERE id=?", [ident], |r| r.get(0)).optional()?;
    let mut extras: Map<String, Value> = match row {
        Some(Some(t)) if !t.is_empty() => serde_json::from_str::<Value>(&t)?.as_object().cloned().unwrap_or_default(),
        _ => Map::new(),
    };
    let before = extras.get(PRESET_KEY).cloned().unwrap_or(Value::Null);
    if truthy(name) {
        extras.insert(PRESET_KEY.into(), name.clone());
    } else {
        extras.shift_remove(PRESET_KEY);
    }
    if !eq(&before, extras.get(PRESET_KEY).unwrap_or(&Value::Null)) {
        db.execute("UPDATE photos SET extras=? WHERE id=?", params![dumps(&Value::Object(extras), false), ident])?;
    }
    Ok(before)
}

/// Port of `photo_service.remember_batch`, inside the write transaction: append a batch, dropping redo entries
/// and the oldest beyond the depth.
pub fn remember_batch(catalog: &Catalog, entries: &[Value], entry: Value) -> Result<()> {
    let state = |e: &Value| get(e, "state").clone();
    let kept: Vec<&Value> = entries.iter().filter(|e| state(e) != "undone").collect();
    let applied: Vec<&Value> = kept.into_iter().filter(|e| state(e) == "applied").collect();
    let excess = applied.len().saturating_sub(MCP_DEPTH - 1);
    let mut dropped: Vec<&Value> = entries.iter().filter(|e| state(e) == "undone").collect();
    dropped.extend(&applied[..excess]);
    let drop_ids: Vec<Value> = dropped.iter().map(|e| get(e, "id").clone()).collect();
    for item in &dropped {
        catalog
            .connection()
            .execute("DELETE FROM preferences WHERE key=?", [format!("{BATCH_PREFIX}{}", text(get(item, "id")))])?;
    }
    let mut index: Vec<Value> =
        entries.iter().filter(|e| !drop_ids.iter().any(|d| eq(d, get(e, "id")))).cloned().collect();
    index.push(entry);
    write_index(catalog, &index)
}

/// Options of [`apply_batch`].
pub struct BatchOptions<'a> {
    /// History label.
    pub label: &'a str,
    /// The tool that made the batch (`apply_edits`, `auto_tone`, …).
    pub tool: &'a str,
    /// Name of the pre-AI snapshot.
    pub snapshot_name: &'a str,
    /// `command_batch.apply`'s switch.
    pub switch: bool,
    /// `{id: preset name}` remembered with the photos the batch changes.
    pub presets: Option<&'a BTreeMap<i64, String>>,
}

/// Port of `photo_service.apply_batch`: one undoable AI batch, `(batch id or None, changed ids)`. Each photo the
/// AI edits first in this app run gets a pre-AI snapshot; `snapshotted` is the service's set of those photos.
pub fn apply_batch(
    catalog: &Catalog,
    records: &[Value],
    values: &[Value],
    options: &BatchOptions<'_>,
    snapshotted: &mut HashSet<i64>,
    tokens: Tokens<'_>,
) -> Result<(Option<String>, Vec<i64>)> {
    let entries = batch_index(catalog)?;
    let taken: Vec<Value> = entries.iter().map(|e| get(e, "id").clone()).collect();
    let mut bid = format!("b{}", tokens()?);
    while taken.iter().any(|t| *t == Value::String(bid.clone())) {
        bid = format!("b{}", tokens()?);
    }
    let mut frozen: BTreeMap<i64, &Value> = BTreeMap::new();
    for record in records {
        frozen.insert(id_of(record)?, record);
    }
    let already: HashSet<i64> = snapshotted.clone();
    let mut after = |ids: &[i64]| -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let db = catalog.connection();
        let now = catalog.now();
        for ident in ids {
            if already.contains(ident) {
                continue;
            }
            let settings = frozen.get(ident).map(|r| get(r, "settings")).unwrap_or(&Value::Null);
            let payload = dumps(&profiles::pack(db, settings, true)?, true);
            db.execute(
                "INSERT INTO snapshots(photo_id,name,edits,created) VALUES(?,?,?,?)",
                params![ident, options.snapshot_name, payload, now],
            )?;
        }
        let mut named = Map::new();
        for ident in ids {
            if let Some(name) = options.presets.and_then(|p| p.get(ident)).filter(|n| !n.is_empty()) {
                let name = Value::String(name.clone());
                let before = set_preset(catalog, *ident, &name)?;
                named.insert(ident.to_string(), json!([before, name]));
            }
        }
        let mut entry = json!({"id": bid, "tool": options.tool, "label": options.label, "count": ids.len(),
                               "created": now, "state": "applied"});
        if !named.is_empty() {
            entry["presets"] = Value::Object(named);
        }
        remember_batch(catalog, &entries, entry)
    };
    if in_transaction(catalog) {
        return Err(Error::Busy("Grainy is saving something else".into()));
    }
    let mut meta = Map::new();
    meta.insert("id".into(), bid.clone().into());
    meta.insert("tool".into(), options.tool.into());
    let apply_options = ApplyOptions {
        label: options.label.into(),
        remember: true,
        batch_key: format!("{BATCH_PREFIX}{bid}"),
        meta: Some(meta),
        compact: true,
        switch: options.switch,
    };
    let ids = batch::apply(catalog, records, values, &apply_options, Some(&mut after))?;
    snapshotted.extend(ids.iter().copied());
    Ok((if ids.is_empty() { None } else { Some(bid) }, ids))
}

/// Port of `photo_service.undo_batch`: undo the most recent applied AI batch (or redo the first undone one);
/// `(id, tool, ids)`. `batch` is the id the caller expects.
pub fn undo_batch(catalog: &Catalog, batch: Option<&str>, redo: bool) -> Result<(Value, String, Vec<i64>)> {
    let mut entries = batch_index(catalog)?;
    let candidate = if redo {
        entries.iter().find(|e| get(e, "state") == "undone")
    } else {
        entries.iter().rev().find(|e| get(e, "state") == "applied")
    };
    let Some(candidate) = candidate.cloned() else {
        return Err(service(if redo { "there is no batch to redo" } else { "there is no batch to undo" }));
    };
    let cid = get(&candidate, "id").clone();
    let tool = match candidate.get("tool") {
        Some(t) => text(t),
        None => String::new(),
    };
    if let Some(batch) = batch {
        if !eq(&Value::String(batch.into()), &cid) {
            return Err(service(format!(
                "{batch} is not the most recent batch; the most recent is {} ({tool})",
                text(&cid)
            )));
        }
    }
    let mut flip = |_: &[i64]| -> Result<()> {
        for entry in entries.iter_mut() {
            if eq(get(entry, "id"), &cid) {
                entry["state"] = (if redo { "applied" } else { "undone" }).into();
            }
        }
        if let Some(presets) = candidate.get("presets").and_then(Value::as_object) {
            for (ident, pair) in presets {
                let ident: i64 =
                    ident.parse().map_err(|_| Error::Value(format!("invalid literal for int(): {ident}")))?;
                let pair = pair.as_array().cloned().unwrap_or_default();
                let value = pair.get(usize::from(redo)).cloned().unwrap_or(Value::Null);
                set_preset(catalog, ident, &value)?;
            }
        }
        write_index(catalog, &entries)
    };
    if in_transaction(catalog) {
        return Err(Error::Busy("Grainy is saving something else".into()));
    }
    let cid_text = text(&cid);
    if tool == "set_labels" {
        let ids = crate::labels::restore(catalog, &cid_text, redo, &mut flip)?;
        return Ok((cid, tool, ids));
    }
    match batch::restore(catalog, redo, &format!("{BATCH_PREFIX}{cid_text}"), Some(&mut flip), Some(&cid)) {
        Ok(ids) => Ok((cid, tool, ids)),
        Err(Error::Conflict { kind: Conflict::Changed | Conflict::History, .. }) => Err(service(format!(
            "{cid_text} can no longer be undone: some of its photos were edited or removed afterwards. \
             Undo those photos one by one in Grainy instead."
        ))),
        Err(Error::Conflict { .. }) => {
            Err(service(if redo { "there is no batch to redo" } else { "there is no batch to undo" }))
        }
        Err(error) => Err(error),
    }
}
