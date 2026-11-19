//! Adapters to the catalog-owned maintenance codec; preserves library exception categories.
use crate::{Error, Result};
use grainy_catalog::{
    profiles::maintenance as shared,
    rusqlite::{types::Value as Sql, Connection},
};
use serde_json::Value;
use std::collections::BTreeSet;
fn error(e: shared::Error) -> Error {
    match e {
        shared::Error::Value(s) => Error::Value(s),
        shared::Error::Type(s) => Error::Type(s),
        shared::Error::Sql(e) => Error::Sql(e),
        shared::Error::Json(e) => Error::Json(e),
        shared::Error::Cancelled => Error::Cancelled,
    }
}
pub(crate) fn loads(db: &Connection, text: &str) -> Result<Value> {
    shared::loads(db, text).map_err(error)
}
pub(crate) fn pack_internal(db: &Connection, v: &Value) -> Result<Value> {
    shared::pack_internal(db, v).map_err(error)
}
pub(crate) fn migrate(db: &Connection) -> Result<()> {
    shared::migrate(db).map_err(error)
}
pub(crate) fn json_cell(cell: Sql) -> Result<Value> {
    shared::json_cell(cell).map_err(error)
}
pub(crate) fn payload_columns(db: &Connection, all: bool) -> Result<Vec<(&'static str, &'static str, &'static str)>> {
    shared::payload_columns(db, all).map_err(error)
}
pub(crate) fn verify_references(db: &Connection, check: &mut dyn FnMut() -> Result<()>) -> Result<BTreeSet<String>> {
    let mut failure = None;
    let result = shared::verify_references(db, &mut || match check() {
        Ok(()) => Ok(()),
        Err(e) => {
            failure = Some(e);
            Err(shared::Error::Cancelled)
        }
    });
    if let Some(e) = failure {
        Err(e)
    } else {
        result.map_err(error)
    }
}
