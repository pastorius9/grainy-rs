//! Port of `luma/adobe_preset.py`: portable Adobe preset sources with partial, target-aware application.

use std::path::Path;

use base64::alphabet::STANDARD as ALPHABET;
use base64::engine::{general_purpose, DecodePaddingMode, Engine, GeneralPurpose, GeneralPurposeConfig};
use grainy_core::{normalized, Settings};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::develop::{convert, from_lua, from_xmp_bytes, summary, Converted, VERSION};
use crate::error::{Error, Result};
use crate::lua::Lua;
use crate::pyutil::WinPath;

/// `adobe_preset.KEY`: where a preset's source record is kept inside stored settings.
pub const KEY: &str = "_adobe_preset";
const LIMIT: usize = 16 * 1024 * 1024;
/// `adobe_preset.GEOMETRY`: per-photo composition that a preset never replaces.
pub const GEOMETRY: [&str; 4] = ["crop", "rotation", "straighten", "flip"];
/// `engine.RAW_EXTENSIONS`.
pub const RAW_EXTENSIONS: [&str; 15] = [
    ".arw", ".cr2", ".cr3", ".nef", ".nrw", ".raf", ".rw2", ".orf", ".pef", ".dng", ".srw", ".3fr", ".iiq", ".kdc",
    ".mos",
];

fn python_base64() -> GeneralPurpose {
    // binascii.a2b_base64(strict): canonical padding required, non-zero trailing bits tolerated.
    GeneralPurpose::new(
        &ALPHABET,
        GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(DecodePaddingMode::RequireCanonical),
    )
}

fn hex_digest(data: &[u8]) -> String {
    Sha256::digest(data).iter().map(|b| format!("{b:02x}")).collect()
}

/// Port of `adobe_preset.values`: the Adobe values stored in a preset record, after the checksum check.
pub fn values(record: &Map<String, Value>) -> Result<Lua> {
    let version_ok = match record.get("version") {
        Some(Value::Number(n)) => n.as_f64() == Some(VERSION as f64),
        Some(Value::Bool(b)) => i64::from(*b) == VERSION,
        _ => false,
    };
    let format = record.get("format").and_then(Value::as_str);
    if !version_ok || !matches!(format, Some("xmp") | Some("lrtemplate")) {
        return Err(Error::data("지원하지 않는 Adobe 프리셋 기록입니다."));
    }
    let encoded = match record.get("data") {
        Some(Value::String(s)) if s.chars().count() <= LIMIT.div_ceil(3) * 4 => s,
        _ => return Err(Error::data("Adobe 프리셋의 원문 크기가 올바르지 않습니다.")),
    };
    let data = python_base64().decode(encoded).map_err(|_| Error::data("Adobe 프리셋 원문을 읽지 못했습니다."))?;
    if data.len() > LIMIT || record.get("sha256").and_then(Value::as_str) != Some(hex_digest(&data).as_str()) {
        return Err(Error::data("Adobe 프리셋 원문의 체크섬이 일치하지 않습니다."));
    }
    if format == Some("xmp") {
        from_xmp_bytes(&data)
    } else {
        let body = data.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&data);
        let text = std::str::from_utf8(body).map_err(|e| Error::value(format!("'utf-8' codec can't decode: {e}")))?;
        from_lua(text)
    }
}

/// Port of `adobe_preset.translate(record, base=None, is_raw=True)`: converts a preset record, keeping the
/// target photo's composition (crop, rotation, straighten, flip).
pub fn translate(record: &Map<String, Value>, base: Option<&Map<String, Value>>, is_raw: bool) -> Result<Converted> {
    let empty = Map::new();
    let original = normalized(base.unwrap_or(&empty));
    let mut result = convert(&values(record)?, Some(&original.0), is_raw, None)?;
    for key in GEOMETRY {
        result.settings.0.insert(key.to_owned(), original.value(key).clone());
    }
    for key in result.mapped.clone() {
        if key == "HasCrop" || key.starts_with("Crop") {
            result.mapped.retain(|k| *k != key);
            result.omitted.set(&key, "사진별 크롭은 프리셋에 적용하지 않습니다.");
        }
    }
    Ok(result)
}

/// Port of `adobe_preset.read`, from the file's bytes and suffix (`xmp` or `lrtemplate`, lower case, no dot):
/// the preset settings with the source record under [`KEY`].
pub fn read_bytes(suffix: &str, data: &[u8]) -> Result<Map<String, Value>> {
    if data.len() > LIMIT {
        return Err(Error::data("Adobe 프리셋 파일이 지원 크기를 넘었습니다."));
    }
    let mut record = Map::new();
    record.insert("version".into(), json!(VERSION));
    record.insert("format".into(), json!(suffix));
    record.insert("data".into(), json!(general_purpose::STANDARD.encode(data)));
    record.insert("sha256".into(), json!(hex_digest(data)));
    let result = translate(&record, None, true)?;
    if result.mapped.is_empty() {
        return Err(Error::data(format!("변환 가능한 프리셋 보정이 없습니다.\n{}", summary(&result, None))));
    }
    let note = "\n\n파일에 지정된 항목만 적용합니다. RAW 색온도는 RAW 사진에만 적용하며 사진별 크롭은 유지합니다.";
    record.insert("report".into(), json!(format!("{}{note}", summary(&result, None))));
    record.insert("preview".into(), json!(format!("{}{note}", summary(&result, Some(5)))));
    let mut out = result.settings.0;
    out.insert(KEY.into(), Value::Object(record));
    Ok(out)
}

/// Port of `adobe_preset.read(path)`.
pub fn read(path: &Path) -> Result<Map<String, Value>> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|e| Error::io(e.to_string()))?;
    let mut data = Vec::new();
    file.take(LIMIT as u64 + 1).read_to_end(&mut data).map_err(|e| Error::io(e.to_string()))?;
    let suffix = WinPath::new(&path.to_string_lossy()).suffix().to_lowercase();
    read_bytes(suffix.trim_start_matches('.'), &data)
}

/// Port of `adobe_preset.resolve(preset, base, path)`: the settings a preset gives for one photo.
pub fn resolve(preset: &Map<String, Value>, base: &Map<String, Value>, path: &str) -> Result<Settings> {
    match preset.get(KEY) {
        None => Ok(normalized(preset)),
        Some(Value::Object(record)) => {
            let suffix = WinPath::new(path).suffix().to_lowercase();
            Ok(translate(record, Some(base), RAW_EXTENSIONS.contains(&suffix.as_str()))?.settings)
        }
        Some(other) => {
            Err(Error::attribute(format!("'{}' object has no attribute 'get'", crate::pyutil::json_type_name(other))))
        }
    }
}
