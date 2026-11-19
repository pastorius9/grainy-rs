//! Port of luma/profile_store.py asset codecs.
pub mod maintenance;
use crate::{invalid, json, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Map;
use serde_json::Value;
use sha2::{Digest, Sha256};
const MAX: usize = 4 * 1024 * 1024;

pub(crate) fn hash(data: impl AsRef<[u8]>) -> String {
    format!("{:x}", Sha256::digest(data.as_ref()))
}
/// Malformed field handling of the existing catalog and service adapters.
#[derive(Clone, Copy)]
pub enum FieldPolicy {
    /// Catalog reports non-string fields as `expected a string`.
    Catalog,
    /// Service treats non-string fields as empty before checksum validation.
    Service,
}
impl FieldPolicy {
    fn string(self, v: &Value) -> Result<&str> {
        match self {
            Self::Catalog => crate::string(v),
            Self::Service => Ok(v.as_str().unwrap_or("")),
        }
    }
}
fn digest(v: &Value, policy: FieldPolicy) -> Result<&str> {
    let s = policy.string(v)?;
    if s.len() != 64 || !s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
        return Err(invalid("invalid camera profile checksum"));
    }
    Ok(s)
}
fn asset(db: &Connection, sha: &str) -> Result<Vec<u8>> {
    let row = asset_row(db, sha)?;
    let data = row.and_then(|r| r.0).ok_or_else(|| invalid("missing or corrupt camera profile asset"))?;
    if hash(&data) != sha {
        return Err(invalid("camera profile asset checksum mismatch"));
    }
    Ok(data)
}
fn walk(db: &Connection, value: &Value, pack: bool, internal: bool, policy: FieldPolicy) -> Result<Value> {
    map_profiles(value, &mut |record| {
        let v = &Value::Object(record.clone());
        let mut p = record.clone();
        if p.contains_key("data_ref") {
            let sha = digest(&v["sha256"], policy)?;
            if v["data_ref"] != sha || p.contains_key("data") {
                return Err(invalid("invalid camera profile reference"));
            }
            if pack && !internal {
                return Err(invalid("external settings must contain profile bytes"));
            }
            let data = asset(db, sha)?;
            if !pack {
                p.shift_remove("data_ref");
                p.insert("data".into(), STANDARD.encode(data).into());
            }
        } else if pack {
            let sha = digest(&v["sha256"], policy)?;
            let encoded = policy.string(&v["data"])?;
            if encoded.len() > MAX.div_ceil(3) * 4 {
                return Err(invalid("camera profile too large"));
            }
            let data = STANDARD.decode(encoded).map_err(|_| invalid("invalid profile base64"))?;
            if data.is_empty() || data.len() > MAX || hash(&data) != sha || STANDARD.encode(&data) != encoded {
                return Err(invalid("camera profile checksum or encoding mismatch"));
            }
            let exists: bool =
                db.query_row("SELECT EXISTS(SELECT 1 FROM profile_assets WHERE sha256=?)", [sha], |r| r.get(0))?;
            if exists {
                if asset(db, sha)? != data {
                    return Err(invalid("existing profile is corrupt"));
                }
            } else {
                db.execute("INSERT INTO profile_assets VALUES(?,?,?)", params![sha, data, data.len() as i64])?;
            }
            p.shift_remove("data");
            p.insert("data_ref".into(), sha.into());
        }
        Ok(p)
    })
}
/// Port of luma/profile_store.py ProfileCodec.pack.
pub fn pack(db: &Connection, v: &Value, internal: bool) -> Result<Value> {
    walk(db, v, true, internal, FieldPolicy::Catalog)
}
/// Port of luma/profile_store.py ProfileCodec.dumps.
pub fn dumps(db: &Connection, v: &Value) -> Result<String> {
    Ok(json::dumps(&pack(db, v, false)?, true, false, false))
}
/// Port of luma/profile_store.py ProfileCodec.loads.
pub fn loads(db: &Connection, s: &str) -> Result<Value> {
    let v = serde_json::from_str(s)?;
    if s.contains("data_ref") {
        walk(db, &v, false, false, FieldPolicy::Catalog)
    } else {
        Ok(v)
    }
}

/// ProfileCodec's per-record transformation, retaining the caller's error type.
pub type ProfileTransform<'a, E> = &'a mut dyn FnMut(&Map<String, Value>) -> std::result::Result<Map<String, Value>, E>;

/// Port of profile_store recursive profile traversal, preserving dictionary order.
pub fn map_profiles<E>(value: &Value, transform: ProfileTransform<'_, E>) -> std::result::Result<Value, E> {
    match value {
        Value::Array(items) => {
            Ok(Value::Array(items.iter().map(|v| map_profiles(v, transform)).collect::<std::result::Result<_, E>>()?))
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                let new = match item {
                    Value::Object(record) if key == "dcp_profile" => Value::Object(transform(record)?),
                    other => map_profiles(other, transform)?,
                };
                out.insert(key.clone(), new);
            }
            Ok(Value::Object(out))
        }
        other => Ok(other.clone()),
    }
}

/// Port of luma/profile_store.py ProfileCodec.expand for ordinary catalog readers.
pub fn expand(db: &Connection, value: &Value) -> Result<Value> {
    walk(db, value, false, false, FieldPolicy::Catalog)
}
/// Port of luma/profile_store.py verify_references, including maintenance cancellation and dust checkpoints.
pub use maintenance::verify_references;

/// Port of ProfileCodec.pack, retaining the caller's malformed-field policy.
pub fn pack_with_policy(db: &Connection, v: &Value, internal: bool, policy: FieldPolicy) -> Result<Value> {
    walk(db, v, true, internal, policy)
}
/// Port of ProfileCodec.loads, retaining the caller's malformed-field policy.
pub fn loads_with_policy(db: &Connection, s: &str, policy: FieldPolicy) -> Result<Value> {
    let v = serde_json::from_str(s)?;
    if s.contains("data_ref") {
        walk(db, &v, false, false, policy)
    } else {
        Ok(v)
    }
}

type AssetRow = (Option<Vec<u8>>, i64);
pub(super) fn asset_row(db: &Connection, sha: &str) -> std::result::Result<Option<AssetRow>, rusqlite::Error> {
    db.query_row(
        "SELECT CASE WHEN typeof(data)='blob' AND size BETWEEN 1 AND ? AND length(data)=size THEN data END,size \
         FROM profile_assets WHERE sha256=?",
        params![MAX as i64, sha],
        |r| Ok((r.get::<_, Option<Vec<u8>>>(0)?, r.get::<_, i64>(1)?)),
    )
    .optional()
}
