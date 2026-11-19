//! Targets (`photo_service` "targets"): list_photos filters, compact list rows, folders, frozen records.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use grainy_catalog::{query, rusqlite, Catalog};
use serde_json::{json, Map, Value};

use crate::pyutil::{get, parent, suffix, truthy};
use crate::{service, Result, LABELS, SORTS, VIDEO_EXTENSIONS};

/// `file_access` states.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// A regular file (or a folder when asked for one).
    Present,
    /// Missing, not a regular file, or not answering.
    Missing,
    /// The system does not let Grainy read it.
    Denied,
}

/// Port of `file_access.access(path)`.
pub fn access(path: &str) -> Access {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => Access::Present,
        Ok(_) => Access::Missing,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => Access::Denied,
        Err(_) => Access::Missing,
    }
}

/// Port of `photo_service.where_options`: `library_query.query` options for list_photos filters.
pub fn where_options(where_: &Value, sort: Option<&str>, descending: bool, allow_empty: bool) -> Result<Value> {
    let empty = Map::new();
    let filters = if truthy(where_) { where_.as_object().unwrap_or(&empty) } else { &empty };
    let w = Value::Object(filters.clone());
    if filters.contains_key("recursive") && !truthy(get(&w, "folder")) {
        return Err(service("recursive needs folder"));
    }
    if filters.is_empty() && !allow_empty {
        return Err(service("where needs at least one filter"));
    }
    if let Some(sort) = sort {
        if !SORTS.contains(&sort) {
            return Err(service(format!("sort must be one of {}", SORTS.join(", "))));
        }
    }
    let search = match filters.get("text") {
        Some(v) if truthy(v) => v.clone(),
        _ => Value::String(String::new()),
    };
    let mut rules = Map::new();
    let mut mode = "all";
    match filters.get("flag") {
        None | Some(Value::Null) => {}
        Some(v) if v == "picked" => mode = "picked",
        Some(v) if v == "rejected" => mode = "rejected",
        Some(v) if v == "unflagged" => {
            rules.insert("flag".into(), 0.into());
        }
        Some(_) => return Err(service("flag must be picked, rejected or unflagged")),
    }
    for key in ["rating_min", "rating_max"] {
        if let Some(v) = filters.get(key).filter(|v| !v.is_null()) {
            rules.insert(key.into(), v.clone());
        }
    }
    if let Some(label) = filters.get("label").filter(|v| !v.is_null()) {
        let Some((_, korean)) = LABELS.iter().find(|(name, _)| label == name) else {
            let names: Vec<&str> = LABELS.iter().map(|(n, _)| *n).collect();
            return Err(service(format!("label must be one of {}", names.join(", "))));
        };
        rules.insert("label".into(), (*korean).into());
    }
    Ok(json!({
        "search": search,
        "folder": get(&w, "folder"),
        "recursive": truthy(get(&w, "recursive")),
        "mode": mode,
        "rules": rules,
        "collection": get(&w, "collection"),
        "collapsed": false,
        "stack_orders": {},
        "sort": sort,
        "descending": descending,
    }))
}

/// Port of `photo_service.query_ids`: ids matching list_photos filters, in list order. Any thread.
pub fn query_ids(
    directory: &Path,
    where_: &Value,
    sort: Option<&str>,
    descending: bool,
    cancel: Option<&Arc<AtomicBool>>,
    allow_empty: bool,
) -> Result<Vec<i64>> {
    let options = where_options(where_, sort, descending, allow_empty)?;
    let fresh = Arc::new(AtomicBool::new(false));
    let cancel = cancel.unwrap_or(&fresh);
    match query::query(directory.join("catalog.sqlite"), &options, cancel, chrono::Local::now().naive_local()) {
        Ok(result) => {
            Ok(result["ids"].as_array().map(|ids| ids.iter().filter_map(Value::as_i64).collect()).unwrap_or_default())
        }
        Err(grainy_catalog::Error::Cancelled) => Err(service("the filter took too long; narrow it")),
        Err(error) => Err(error.into()),
    }
}

fn label_name(label: &Value) -> Value {
    LABELS
        .iter()
        .find(|(_, korean)| label == korean)
        .map_or_else(|| label.clone(), |(name, _)| Value::String((*name).into()))
}

/// Port of `photo_service.page_rows`: `(dirs, rows)` for ids in order, compact rows without zero/empty fields.
pub fn page_rows(directory: &Path, ids: &[i64]) -> Result<(Vec<String>, Vec<Value>)> {
    if ids.is_empty() {
        return Ok((vec![], vec![]));
    }
    let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let db = rusqlite::Connection::open_with_flags(directory.join("catalog.sqlite"), flags)?;
    db.busy_timeout(std::time::Duration::from_secs(2))?;
    let mut found: std::collections::HashMap<i64, Value> = std::collections::HashMap::new();
    for part in ids.chunks(500) {
        let marks = vec!["?"; part.len()].join(",");
        let mut statement = db.prepare(&format!(
            "SELECT id,path,name,rating,flag,label,virtual_source FROM photos WHERE id IN ({marks})"
        ))?;
        let rows = statement
            .query_map(rusqlite::params_from_iter(part), grainy_catalog::row_value)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for r in rows {
            found.insert(crate::id_of(&r)?, r);
        }
    }
    let (mut dirs, mut rows) = (Vec::<String>::new(), Vec::new());
    for ident in ids {
        let Some(r) = found.get(ident) else { continue };
        let path = crate::text_of(r, "path")?;
        let folder = parent(path);
        let position = match dirs.iter().position(|d| *d == folder) {
            Some(p) => p,
            None => {
                dirs.push(folder);
                dirs.len() - 1
            }
        };
        let mut row = Map::new();
        row.insert("id".into(), (*ident).into());
        row.insert("name".into(), get(r, "name").clone());
        if truthy(get(r, "rating")) {
            row.insert("r".into(), r["rating"].clone());
        }
        if truthy(get(r, "flag")) {
            row.insert("f".into(), r["flag"].clone());
        }
        if truthy(get(r, "label")) {
            row.insert("l".into(), label_name(&r["label"]));
        }
        if !get(r, "virtual_source").is_null() {
            row.insert("v".into(), 1.into());
        }
        if position != 0 {
            row.insert("d".into(), position.into());
        }
        if access(path) == Access::Missing {
            row.insert("x".into(), 1.into());
        }
        rows.push(Value::Object(row));
    }
    Ok((dirs, rows))
}

/// Port of `photo_service.folder_rows`: `[{path, n}]` for every folder with photos, optionally only those inside
/// `under`. Refreshes the derived browser index like the library list does.
pub fn folder_rows(directory: &Path, under: Option<&str>, cancel: Option<&AtomicBool>) -> Result<Vec<Value>> {
    let db = rusqlite::Connection::open(directory.join("catalog.sqlite"))?;
    db.busy_timeout(std::time::Duration::from_secs(10))?;
    let fresh = AtomicBool::new(false);
    match query::sync_index(&db, cancel.unwrap_or(&fresh)) {
        Ok(()) => {}
        Err(grainy_catalog::Error::Cancelled) => return Err(service("the folder list took too long")),
        Err(error) => return Err(error.into()),
    }
    let mut statement = db.prepare(
        "SELECT MIN(folder) folder,COUNT(*) count FROM browser_index GROUP BY folder_key ORDER BY MIN(folder)",
    )?;
    let rows = statement
        .query_map([], |r| Ok(json!({"path": r.get::<_, String>(0)?, "n": r.get::<_, i64>(1)?})))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let platform = grainy_catalog::folders::Platform::native();
    Ok(rows
        .into_iter()
        .filter(|r| under.is_none_or(|u| platform.contains_folder(u, r["path"].as_str().unwrap_or(""))))
        .collect())
}

/// Port of `photo_service.freeze`: `(records, errors)`, the photos as they are now.
pub fn freeze(catalog: &Catalog, ids: &[i64]) -> Result<(Vec<Value>, Vec<Value>)> {
    let (mut records, mut errors) = (vec![], vec![]);
    for ident in ids {
        let Some(row) = catalog.photo(*ident)? else {
            errors.push(json!({"id": ident, "error": "not in the catalog"}));
            continue;
        };
        let path = crate::text_of(&row, "path")?;
        if VIDEO_EXTENSIONS.contains(&suffix(path).to_lowercase().as_str()) {
            errors.push(json!({"id": ident, "error": "video files cannot be edited"}));
            continue;
        }
        records.push(json!({"id": ident, "path": path, "name": get(&row, "name"), "settings": get(&row, "settings"),
                            "info": get(&row, "info")}));
    }
    Ok((records, errors))
}
