//! Data-only ports of luma/dust_store.py. Review transactions belong to grainy-service.
use crate::{invalid, Result};
use flate2::{write::ZlibEncoder, Compression, Decompress, FlushDecompress, Status};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::Write,
    path::{Path, PathBuf},
};
use unicode_casefold::UnicodeCaseFold;
/// Port of luma/dust_store.py VERSION.
pub const VERSION: u32 = 1;
/// Port of luma/dust_store.py MAX_ENTRY_BYTES.
pub const MAX_ENTRY_BYTES: usize = 16 * 1024 * 1024;
/// Port of luma/dust_store.py unpack; rejects trailing streams, truncation and oversized payloads.
pub fn unpack(payload: &[u8]) -> Result<String> {
    if payload.len() > MAX_ENTRY_BYTES {
        return Err(invalid("compressed payload too large"));
    }
    let mut decoder = Decompress::new(true);
    let mut raw = vec![0u8; MAX_ENTRY_BYTES + 1];
    let status =
        decoder.decompress(payload, &mut raw, FlushDecompress::Finish).map_err(|_| invalid("damaged zlib payload"))?;
    if decoder.total_out() > MAX_ENTRY_BYTES as u64
        || status != Status::StreamEnd
        || decoder.total_in() != payload.len() as u64
    {
        return Err(invalid("truncated, trailing or oversized zlib payload"));
    }
    raw.truncate(decoder.total_out() as usize);
    String::from_utf8(raw).map_err(|_| invalid("payload is not UTF-8"))
}
/// Port of luma/dust_store.py DustReviewStore.encode's data format: UTF-8 JSON in zlib level 3.
/// Review validation and profile-asset externalization are owned by the service/catalog layer.
pub fn pack(value: &Value) -> Result<Vec<u8>> {
    let mut value = value.clone();
    if value.get("state").and_then(Value::as_str) == Some("scanning") {
        value["state"] = json!("pending");
    }
    let raw = grainy_catalog::json::dumps(&value, false, false, false);
    if raw.len() > MAX_ENTRY_BYTES {
        return Err(invalid("JSON payload too large"));
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::new(3));
    encoder.write_all(raw.as_bytes())?;
    let payload = encoder.finish()?;
    if payload.len() > MAX_ENTRY_BYTES {
        return Err(invalid("compressed payload too large"));
    }
    Ok(payload)
}
// Shared catalog's equivalent is private. Path.resolve(strict=False) resolves an existing ancestor.
fn resolved_path(path: &Path) -> Result<PathBuf> {
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
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {
                let (Some(name), Some(parent)) = (ancestor.file_name(), ancestor.parent()) else {
                    return Ok(absolute);
                };
                tail.push(name.to_owned());
                ancestor = parent;
            }
            Err(e) => return Err(e.into()),
        }
    }
}
/// Port of luma/dust_store.py target_key. Folder paths use native Path.resolve and full Unicode casefold.
pub fn target_key(snapshot: &Value, scope: &str) -> Result<String> {
    let target = if scope == "folder" && snapshot.get("folder").and_then(Value::as_str).is_some_and(|s| !s.is_empty()) {
        let path = resolved_path(Path::new(snapshot["folder"].as_str().ok_or_else(|| invalid("folder"))?))?;
        let text = path.to_string_lossy();
        let text = if let Some(s) = text.strip_prefix("\\\\?\\UNC\\") {
            format!("\\\\{s}")
        } else {
            text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
        };
        json!(text.case_fold().collect::<String>())
    } else {
        let ids = snapshot
            .get("scopes")
            .and_then(|v| v.get(scope))
            .and_then(Value::as_array)
            .ok_or_else(|| invalid("scope IDs"))?;
        let mut ids: Vec<_> =
            ids.iter().map(|v| v.as_i64().ok_or_else(|| invalid("scope ID"))).collect::<Result<_>>()?;
        ids.sort_unstable();
        ids.dedup();
        json!(ids)
    };
    let text = grainy_catalog::json::dumps(&json!([scope, target]), false, false, false);
    Ok(format!("{:x}", Sha256::digest(text.as_bytes())))
}
