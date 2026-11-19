//! Port of `luma/photo_labels.py`: atomic rating/flag/label batches that share the AI undo stack without
//! changing develop histories.

use grainy_catalog::{rusqlite, workflow::text_hash, Catalog};
use rusqlite::{params, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::batch::{immediate, in_transaction, undo_text};
use crate::pyutil::{eq, get};
use crate::stack::{batch_index, remember_batch, Tokens};
use crate::{profiles, service, Error, Result, BATCH_PREFIX};

/// Port of `photo_labels.FIELDS`.
pub const FIELDS: [&str; 3] = ["rating", "flag", "label"];

/// One entry of [`apply`]: `{'ids': [...], 'fields': {rating|flag|label: value}}`.
#[derive(Clone, Debug, Default)]
pub struct LabelEntry {
    /// The photos.
    pub ids: Vec<i64>,
    /// The values to set.
    pub fields: Map<String, Value>,
}

/// `(batch id or None, changed ids, unchanged count, errors)` of [`apply`].
pub type Applied = (Option<String>, Vec<i64>, usize, Vec<Value>);

fn labels_row(catalog: &Catalog, ident: i64) -> Result<Option<Value>> {
    Ok(catalog
        .connection()
        .query_row("SELECT rating,flag,label FROM photos WHERE id=?", [ident], grainy_catalog::row_value)
        .optional()?)
}

/// Port of `photo_labels.guard`: `[path, hash of edits, hash of the undo state]`, or `None` for a missing photo.
pub fn guard(catalog: &Catalog, ident: i64) -> Result<Value> {
    let row: Option<(String, String)> = catalog
        .connection()
        .query_row("SELECT path,edits FROM photos WHERE id=?", [ident], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    Ok(match row {
        None => Value::Null,
        Some((path, edits)) => json!([path, text_hash(&edits), text_hash(&undo_text(catalog, ident)?)]),
    })
}

/// Port of `photo_labels.apply`: one undoable label batch.
pub fn apply(catalog: &Catalog, entries: &[LabelEntry], tokens: Tokens<'_>) -> Result<Applied> {
    if in_transaction(catalog) {
        return Err(Error::Busy("Grainy is saving something else".into()));
    }
    let mut targets: Vec<i64> = Vec::new();
    for ident in entries.iter().flat_map(|e| e.ids.iter()) {
        if !targets.contains(ident) {
            targets.push(*ident);
        }
    }
    let mut rows: Vec<(i64, Value)> = Vec::new();
    for ident in &targets {
        if let Some(row) = labels_row(catalog, *ident)? {
            rows.push((*ident, row));
        }
    }
    let mut values: Vec<(i64, Value)> = rows.clone();
    for entry in entries {
        for ident in &entry.ids {
            if let Some((_, value)) = values.iter_mut().find(|(i, _)| i == ident) {
                for (k, v) in &entry.fields {
                    value[k] = v.clone();
                }
            }
        }
    }
    let changed: Vec<i64> =
        rows.iter().zip(&values).filter(|((_, a), (_, b))| !eq(a, b)).map(|((i, _), _)| *i).collect();
    let errors: Vec<Value> = targets
        .iter()
        .filter(|i| !rows.iter().any(|(r, _)| r == *i))
        .map(|i| json!({"id": i, "error": "not in the catalog"}))
        .collect();
    if changed.is_empty() {
        return Ok((None, vec![], rows.len(), errors));
    }
    let index = batch_index(catalog)?;
    let taken: Vec<Value> = index.iter().map(|e| get(e, "id").clone()).collect();
    let mut bid = format!("b{}", tokens()?);
    while taken.iter().any(|t| *t == Value::String(bid.clone())) {
        bid = format!("b{}", tokens()?);
    }
    let lookup = |list: &[(i64, Value)], ident: i64| list.iter().find(|(i, _)| *i == ident).map(|(_, v)| v.clone());
    immediate(catalog, || {
        let db = catalog.connection();
        let mut records = Vec::new();
        for ident in &changed {
            records.push(json!({"id": ident, "before": lookup(&rows, *ident), "after": lookup(&values, *ident),
                                "guard": guard(catalog, *ident)?}));
        }
        for ident in &changed {
            let value = lookup(&values, *ident).unwrap_or(Value::Null);
            let fields = FIELDS
                .iter()
                .map(|k| grainy_catalog::sql_value(get(&value, k)))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            db.execute(
                "UPDATE photos SET rating=?,flag=?,label=? WHERE id=?",
                params![fields[0], fields[1], fields[2], ident],
            )?;
        }
        let group = json!({"id": bid, "state": "applied", "kind": "labels", "records": records});
        db.execute(
            "INSERT OR REPLACE INTO preferences VALUES(?,?)",
            params![format!("{BATCH_PREFIX}{bid}"), profiles::dumps(db, &group)?],
        )?;
        remember_batch(
            catalog,
            &index,
            json!({"id": bid, "tool": "set_labels", "label": "AI 분류", "count": changed.len(),
                   "created": catalog.now(), "state": "applied"}),
        )
    })?;
    let unchanged = rows.len() - changed.len();
    Ok((Some(bid), changed, unchanged, errors))
}

/// Port of `photo_labels.restore`: undo or redo a label batch; `flip` runs inside the transaction.
pub fn restore(
    catalog: &Catalog,
    bid: &str,
    redo: bool,
    flip: &mut dyn FnMut(&[i64]) -> Result<()>,
) -> Result<Vec<i64>> {
    let key = format!("{BATCH_PREFIX}{bid}");
    let mut group = catalog.preference(&key, json!({}))?;
    let (side, target) = if redo { ("before", "after") } else { ("after", "before") };
    if get(&group, "state") != (if redo { "undone" } else { "applied" }) {
        return Err(service(if redo { "there is no batch to redo" } else { "there is no batch to undo" }));
    }
    immediate(catalog, || {
        let db = catalog.connection();
        let records =
            group.get("records").and_then(Value::as_array).cloned().ok_or_else(|| Error::Key("'records'".into()))?;
        for record in &records {
            let ident = crate::id_of(record)?;
            let row = labels_row(catalog, ident)?;
            let same = row.as_ref().is_some_and(|r| eq(r, get(record, side)));
            if !same || !eq(&guard(catalog, ident)?, get(record, "guard")) {
                return Err(service(format!(
                    "{bid} can no longer be undone: some photos or labels changed afterwards"
                )));
            }
        }
        for record in &records {
            let value = get(record, target);
            let fields = FIELDS
                .iter()
                .map(|k| grainy_catalog::sql_value(get(value, k)))
                .collect::<std::result::Result<Vec<_>, _>>()?;
            db.execute(
                "UPDATE photos SET rating=?,flag=?,label=? WHERE id=?",
                params![fields[0], fields[1], fields[2], crate::id_of(record)?],
            )?;
        }
        group["state"] = (if redo { "applied" } else { "undone" }).into();
        db.execute("INSERT OR REPLACE INTO preferences VALUES(?,?)", params![key, profiles::dumps(db, &group)?])?;
        let ids: Vec<i64> = records.iter().map(crate::id_of).collect::<Result<_>>()?;
        flip(&ids)?;
        Ok(ids)
    })
}
