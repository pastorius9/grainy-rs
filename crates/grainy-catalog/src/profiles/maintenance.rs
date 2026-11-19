//! Port of the parts of `luma/profile_store.py` that catalogue maintenance and backup restore need
//! (maintenance validation policy): content-addressed DCP assets stored in `profile_assets`.

/// Errors of luma/profile_store.py maintenance, preserving Python exception categories.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// ValueError from profile validation.
    #[error("{0}")]
    Value(String),
    /// TypeError from malformed stored data.
    #[error("{0}")]
    Type(String),
    /// SQLite failure.
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),
    /// JSON decoding failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Cooperative cancellation.
    #[error("cancelled")]
    Cancelled,
}
impl Error {
    fn value(message: impl Into<String>) -> Self {
        Self::Value(message.into())
    }
}
/// Result of profile maintenance operations.
pub type Result<T> = std::result::Result<T, Error>;
use super::hash as sha_of;
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, types::Value as Sql, Connection, OptionalExtension};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::io::Read;

const MAX_BYTES: usize = 4 * 1024 * 1024;
const REF: &str = "data_ref";
const MAX_ENTRY_BYTES: u64 = 16 * 1024 * 1024;

fn b64_encode(data: &[u8]) -> String {
    STANDARD.encode(data)
}

/// Strict base64 decoding (`base64.b64decode(validate=True)`).
fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let value = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let mut out = Vec::new();
    for (index, chunk) in bytes.chunks(4).enumerate() {
        let last = (index + 1) * 4 == bytes.len();
        let padding = chunk.iter().rev().take_while(|c| **c == b'=').count();
        if padding > 2 || (padding > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let v = if i >= 4 - padding {
                if *c == b'=' {
                    0
                } else {
                    return None;
                }
            } else {
                value(*c)?
            };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if padding < 2 {
            out.push((n >> 8) as u8);
        }
        if padding < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// `profile_store.digest`.
fn digest(value: Option<&Value>) -> Result<&str> {
    match value {
        Some(Value::String(s))
            if s.len() == 64 && s.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) =>
        {
            Ok(s)
        }
        _ => Err(Error::value("카메라 프로파일의 체크섬이 올바르지 않습니다.")),
    }
}

fn short(sha: &str) -> &str {
    sha.get(..12).unwrap_or(sha)
}

/// `profile_store.decoded` (without the cache).
fn decoded(encoded: Option<&Value>, sha: &str) -> Result<Vec<u8>> {
    let text = match encoded {
        Some(Value::String(s)) => s,
        // `lru_cache` hashes its arguments first: a list or a dict is a TypeError there.
        Some(Value::Array(_)) => return Err(Error::Type("unhashable type: 'list'".into())),
        Some(Value::Object(_)) => return Err(Error::Type("unhashable type: 'dict'".into())),
        _ => return Err(Error::value("카메라 프로파일의 크기가 올바르지 않습니다.")),
    };
    if text.len() > MAX_BYTES.div_ceil(3) * 4 {
        return Err(Error::value("카메라 프로파일의 크기가 올바르지 않습니다."));
    }
    let data = b64_decode(text).ok_or_else(|| Error::value("카메라 프로파일을 읽을 수 없습니다."))?;
    if data.is_empty() || data.len() > MAX_BYTES || sha_of(&data) != sha {
        return Err(Error::value("카메라 프로파일의 원본과 체크섬이 일치하지 않습니다."));
    }
    if b64_encode(&data) != *text {
        return Err(Error::value("카메라 프로파일의 인코딩이 올바르지 않습니다."));
    }
    Ok(data)
}

/// `ProfileCodec._asset_row`: the data (None when invalid) and the stored size.
use super::asset_row;

/// `ProfileCodec._read`: the validated asset as base64 text.
pub fn read_asset(db: &Connection, sha: &str) -> Result<String> {
    let row = match asset_row(db, sha) {
        Ok(row) => row,
        Err(rusqlite::Error::SqliteFailure(_, _)) => {
            return Err(Error::value("카탈로그의 카메라 프로파일 보관함이 없습니다."))
        }
        Err(e) => return Err(e.into()),
    };
    let Some((data, size)) = row else {
        return Err(Error::value(format!("카탈로그의 카메라 프로파일 원본이 누락되었습니다: {}", short(sha))));
    };
    match data {
        Some(data)
            if !data.is_empty() && data.len() <= MAX_BYTES && data.len() as i64 == size && sha_of(&data) == sha =>
        {
            Ok(b64_encode(&data))
        }
        _ => Err(Error::value(format!("카탈로그의 카메라 프로파일 원본이 손상되었습니다: {}", short(sha)))),
    }
}

fn walk(value: &Value, transform: super::ProfileTransform<'_, Error>) -> Result<Value> {
    super::map_profiles(value, transform)
}

/// `ProfileCodec.expand`: references replaced by the stored bytes (base64 under `data`).
pub fn expand(db: &Connection, value: &Value) -> Result<Value> {
    walk(value, &mut |record| {
        if !record.contains_key(REF) {
            return Ok(record.clone());
        }
        let sha = digest(record.get("sha256"))?;
        if record[REF] != Value::String(sha.into()) || record.contains_key("data") {
            return Err(Error::value("카메라 프로파일 참조가 올바르지 않습니다."));
        }
        let mut out: Map<String, Value> =
            record.iter().filter(|(k, _)| *k != REF).map(|(k, v)| (k.clone(), v.clone())).collect();
        out.insert("data".into(), Value::String(read_asset(db, sha)?));
        Ok(out)
    })
}

/// `ProfileCodec.loads`: parse stored JSON and expand references.
pub fn loads(db: &Connection, text: &str) -> Result<Value> {
    let value: Value = serde_json::from_str(text)?;
    if text.contains(REF) {
        expand(db, &value)
    } else {
        Ok(value)
    }
}

/// `ProfileCodec.pack(value, internal=True)`: the bytes move into `profile_assets`, references stay.
pub fn pack_internal(db: &Connection, value: &Value) -> Result<Value> {
    let mut assets: Vec<(String, Vec<u8>)> = Vec::new();
    let packed = walk(value, &mut |record| {
        if record.contains_key(REF) {
            // Validate existing references even in a mixed legacy payload.
            let mut probe = Map::new();
            probe.insert("dcp_profile".into(), Value::Object(record.clone()));
            expand(db, &Value::Object(probe))?;
            return Ok(record.clone());
        }
        let sha = digest(record.get("sha256"))?.to_owned();
        let data = decoded(record.get("data"), &sha)?;
        assets.push((sha.clone(), data));
        let mut out: Map<String, Value> =
            record.iter().filter(|(k, _)| *k != "data").map(|(k, v)| (k.clone(), v.clone())).collect();
        out.insert(REF.into(), Value::String(sha));
        Ok(out)
    })?;
    let mut seen = BTreeSet::new();
    for (sha, data) in assets {
        if !seen.insert(sha.clone()) {
            continue;
        }
        match asset_row(db, &sha)? {
            None => {
                db.execute("INSERT INTO profile_assets VALUES(?,?,?)", params![sha, data, data.len() as i64])?;
            }
            Some((existing, size)) => {
                if existing.as_deref() != Some(&data[..]) || size != data.len() as i64 {
                    return Err(Error::value(format!("기존 카메라 프로파일 원본이 손상되었습니다: {}", short(&sha))));
                }
            }
        }
    }
    Ok(packed)
}

/// `profile_store.migrate`.
pub fn migrate(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS profile_assets (sha256 TEXT PRIMARY KEY, data BLOB NOT NULL, size INTEGER NOT NULL)",
    )?;
    Ok(())
}

/// `profile_store.payload_columns`: (table, column, where clause).
pub fn payload_columns(
    db: &Connection,
    all_preferences: bool,
) -> Result<Vec<(&'static str, &'static str, &'static str)>> {
    let mut stmt = db.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
    let tables: BTreeSet<String> = stmt.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
    let mut result = Vec::new();
    for table in ["photos", "history", "presets", "snapshots"] {
        if tables.contains(table) {
            result.push((table, "edits", ""));
        }
    }
    if tables.contains("preferences") {
        result.push((
            "preferences",
            "value",
            if all_preferences { "" } else { " WHERE key LIKE 'undo:%' OR key='raw_defaults_v1'" },
        ));
    }
    Ok(result)
}

/// `json.loads(cell)` of a SQLite value, with Python's `TypeError` for a non-text cell.
pub fn json_cell(cell: Sql) -> Result<Value> {
    match cell {
        Sql::Text(text) => Ok(serde_json::from_str(&text)?),
        Sql::Blob(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Sql::Null => Err(Error::Type("the JSON object must be str, bytes or bytearray, not NoneType".into())),
        Sql::Integer(_) => Err(Error::Type("the JSON object must be str, bytes or bytearray, not int".into())),
        Sql::Real(_) => Err(Error::Type("the JSON object must be str, bytes or bytearray, not float".into())),
    }
}

/// `profile_store.references`.
pub fn references(value: &Value) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    walk(value, &mut |record| {
        if record.contains_key(REF) {
            let sha = digest(record.get("sha256"))?;
            if record[REF] != Value::String(sha.into()) || record.contains_key("data") {
                return Err(Error::value("카메라 프로파일 참조가 올바르지 않습니다."));
            }
            found.insert(sha.to_owned());
        }
        Ok(record.clone())
    })?;
    Ok(found)
}

/// `dust_store.unpack`.
fn dust_unpack(payload: &[u8]) -> std::result::Result<String, ()> {
    if payload.len() as u64 > MAX_ENTRY_BYTES {
        return Err(());
    }
    let mut input: &[u8] = payload;
    let mut raw = Vec::new();
    {
        let decoder = flate2::bufread::ZlibDecoder::new(&mut input);
        decoder.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut raw).map_err(|_| ())?;
    }
    if raw.len() as u64 > MAX_ENTRY_BYTES || !input.is_empty() {
        return Err(());
    }
    String::from_utf8(raw).map_err(|_| ())
}

/// `profile_store.verify_references`: all reachable internal assets, including unknown preferences.
pub fn verify_references(db: &Connection, check: &mut dyn FnMut() -> Result<()>) -> Result<BTreeSet<String>> {
    let mut found = BTreeSet::new();
    for (table, column, filter) in payload_columns(db, true)? {
        let mut stmt = db.prepare(&format!("SELECT {column} FROM {table}{filter}"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            check()?;
            found.extend(references(&json_cell(row.get::<_, Sql>(0)?)?)?);
        }
    }
    let has_dust: bool = db
        .query_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='dust_entries'", [], |_| Ok(true))
        .optional()?
        .unwrap_or(false);
    if has_dust {
        let mut stmt = db.prepare("SELECT payload FROM dust_entries")?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            check()?;
            let payload = match row.get::<_, Sql>(0)? {
                Sql::Blob(b) => b,
                Sql::Text(t) => t.into_bytes(),
                _ => return Err(Error::Type("object of type 'NoneType' has no len()".into())),
            };
            match dust_unpack(&payload).ok().and_then(|text| serde_json::from_str::<Value>(&text).ok()) {
                Some(value) => found.extend(references(&value)?),
                None => {
                    // A damaged checkpoint can conceal references: keep every asset until it is redetected.
                    let mut all = db.prepare("SELECT sha256 FROM profile_assets")?;
                    let shas: Vec<String> = all.query_map([], |r| r.get(0))?.collect::<std::result::Result<_, _>>()?;
                    found.extend(shas);
                }
            }
        }
    }
    for sha in &found {
        check()?;
        read_asset(db, sha)?;
    }
    Ok(found)
}
