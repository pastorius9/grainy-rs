//! Adapters to the shared catalog profile codec, preserving service field errors.
use crate::Result;
use grainy_catalog::{
    json,
    profiles::{self as shared, FieldPolicy},
    rusqlite::Connection,
};
use serde_json::Value;
pub(crate) fn pack(db: &Connection, v: &Value, internal: bool) -> Result<Value> {
    Ok(shared::pack_with_policy(db, v, internal, FieldPolicy::Service)?)
}
pub(crate) fn dumps(db: &Connection, v: &Value) -> Result<String> {
    let text = json::dumps(v, true, false, false);
    if !text.contains("\"dcp_profile\": {") {
        return Ok(text);
    }
    Ok(json::dumps(&pack(db, v, false)?, true, false, false))
}
pub(crate) fn loads(db: &Connection, text: &str) -> Result<Value> {
    Ok(shared::loads_with_policy(db, text, FieldPolicy::Service)?)
}
