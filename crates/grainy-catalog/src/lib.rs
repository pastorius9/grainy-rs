//! Storage port of `luma/catalog.py`, `folders.py`, `library_query.py`, and `preview_store.py`.
//! All filesystem access is rooted in the explicitly supplied catalog directory.

mod catalog;
pub mod folders;
pub mod json;
pub mod preview;
pub mod profiles;
pub mod query;
mod schema;
pub mod transaction;
pub mod workflow;

pub use catalog::{Catalog, Sources};
pub use rusqlite;
use serde_json::Value;

/// Errors returned by ports of `luma/catalog.py` and its storage helpers.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// SQLite failure from `Catalog` operations.
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    /// Filesystem failure from `Catalog.backup` or preview storage.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Invalid JSON in an oracle catalog payload.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Invalid input, equivalent to Python ValueError.
    #[error("{0}")]
    Invalid(String),
    /// `library_query.QueryCancelled`.
    #[error("query cancelled")]
    Cancelled,
}

/// Result of a ported oracle storage operation.
pub type Result<T> = std::result::Result<T, Error>;

fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

pub(crate) fn path_text(path: &std::path::Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
    }
}
// Path.resolve(strict=False): resolve an existing ancestor even when the leaf is offline.
pub(crate) fn resolved_path(path: &std::path::Path) -> Result<std::path::PathBuf> {
    let absolute = std::path::absolute(path)?;
    let mut ancestor = absolute.as_path();
    let mut tail = Vec::new();
    loop {
        match std::fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for part in tail.into_iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {
                let (Some(name), Some(parent)) = (ancestor.file_name(), ancestor.parent()) else { return Ok(absolute) };
                tail.push(name.to_owned());
                ancestor = parent;
            }
            Err(error) => return Err(error.into()),
        }
    }
}
fn object(v: &Value) -> Result<&serde_json::Map<String, Value>> {
    v.as_object().ok_or_else(|| invalid("expected a JSON object"))
}
fn string(v: &Value) -> Result<&str> {
    v.as_str().ok_or_else(|| invalid("expected a string"))
}
fn integer(v: &Value) -> Result<i64> {
    v.as_i64().ok_or_else(|| invalid("expected an integer"))
}
/// Stored settings JSON as the oracle reads it: `engine.normalized`, keys in the defaults' order.
pub fn settings(v: &Value) -> Result<grainy_core::Settings> {
    let normalized = grainy_core::normalized(object(v)?);
    let order: Value = serde_json::from_str(include_str!("../assets/default_order.json"))?;
    let mut map = serde_json::Map::new();
    for key in order.as_array().ok_or_else(|| invalid("invalid embedded settings order"))? {
        let key = string(key)?;
        if let Some(v) = normalized.0.get(key) {
            map.insert(key.into(), v.clone());
        }
    }
    Ok(grainy_core::Settings(map))
}

/// One SQLite row as a JSON object (column name to value), as the oracle's `sqlite3.Row` dicts.
pub fn row_value(row: &rusqlite::Row<'_>) -> rusqlite::Result<Value> {
    use rusqlite::types::ValueRef;
    let mut map = serde_json::Map::new();
    for i in 0..row.as_ref().column_count() {
        let v = match row.get_ref(i)? {
            ValueRef::Null => Value::Null,
            ValueRef::Integer(v) => v.into(),
            ValueRef::Real(v) => v.into(),
            ValueRef::Text(v) => String::from_utf8_lossy(v).into_owned().into(),
            ValueRef::Blob(v) => Value::Array(v.iter().map(|v| (*v).into()).collect()),
        };
        map.insert(row.as_ref().column_name(i)?.into(), v);
    }
    Ok(map.into())
}
/// A JSON scalar as the SQLite value Python's sqlite3 would bind (bool as integer).
pub fn sql_value(v: &Value) -> Result<rusqlite::types::Value> {
    use rusqlite::types::Value as S;
    Ok(match v {
        Value::Null => S::Null,
        Value::Bool(v) => S::Integer(i64::from(*v)),
        Value::Number(v) => {
            if let Some(v) = v.as_i64() {
                S::Integer(v)
            } else {
                S::Real(v.as_f64().ok_or_else(|| invalid("invalid SQL number"))?)
            }
        }
        Value::String(v) => S::Text(v.clone()),
        _ => return Err(invalid("SQLite field values must be scalar")),
    })
}
