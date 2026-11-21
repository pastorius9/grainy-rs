//! Port of `luma/quick_export.py`: quick export presets and lossless original copies with Grainy XMP settings.

use crate::pyformat::{self, Arg};
use crate::xmp::write_sidecar;
use crate::{pathx, Error, Result};
use grainy_catalog::folders::Platform;
use grainy_catalog::Catalog;
use serde_json::{json, Map, Value};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;

/// Catalogue preference holding the previous export settings.
pub const PREFERENCE: &str = "previous_photo_export";
/// `quick_export.FORMATS`.
pub const FORMATS: [&str; 10] = [
    "JPEG",
    "AVIF",
    "JPEG XL",
    "PNG",
    "PNG 16-bit",
    "TIFF 16-bit",
    "TIFF 8-bit",
    "TIFF HDR 32-bit",
    "Original",
    "AVIF HDR 10-bit",
];

/// `quick_export.SUFFIXES`: the file suffix of a format (`Original` has none).
pub fn suffix(format: &str) -> Option<&'static str> {
    Some(match format {
        "JPEG" => ".jpg",
        "AVIF" | "AVIF HDR 10-bit" => ".avif",
        "JPEG XL" => ".jxl",
        "PNG" | "PNG 16-bit" => ".png",
        "TIFF 16-bit" | "TIFF 8-bit" | "TIFF HDR 32-bit" => ".tif",
        _ => return None,
    })
}

/// Port of `quick_export.preset`: `small`, `large` or `original`; anything else is a `ValueError(kind)`.
pub fn preset(kind: &str) -> Result<Map<String, Value>> {
    if !["small", "large", "original"].contains(&kind) {
        return Err(Error::value(kind));
    }
    let value = json!({
        "format": if kind == "original" { "Original" } else { "JPEG" },
        "quality": if kind == "small" { 80 } else { 100 },
        "longest": if kind == "small" { 2048 } else { 0 },
        "color_space": "sRGB",
        "keep_metadata": false,
        "naming": "{stem}",
        "grayscale": true,
        "dither": true,
    });
    Ok(value.as_object().cloned().unwrap_or_default())
}

fn is_int_in(value: &Value, low: i64, high: i64) -> bool {
    value.as_i64().is_some_and(|n| (low..=high).contains(&n))
}

/// Port of `quick_export.previous`: the saved export settings, or `None` when absent or not valid. `hdr_feature`
/// is `engine.HDR_FEATURE` (whether 32-bit HDR TIFF export is available).
pub fn previous(catalog: &Catalog, hdr_feature: bool) -> Result<Option<Map<String, Value>>> {
    let value = catalog.preference(PREFERENCE, Value::Null)?;
    Ok(validate_previous(&value, hdr_feature))
}

/// The validation of [`previous`] on a stored value.
pub fn validate_previous(value: &Value, hdr_feature: bool) -> Option<Map<String, Value>> {
    let object = value.as_object()?;
    let mut result = Map::new();
    for key in ["folder", "format", "quality", "longest", "color_space", "keep_metadata", "naming"] {
        result.insert(key.into(), object.get(key)?.clone());
    }
    if !result["folder"].as_str().is_some_and(|s| !s.is_empty()) {
        return None;
    }
    let format = result["format"].as_str()?;
    if !FORMATS.contains(&format) || (matches!(format, "TIFF HDR 32-bit" | "AVIF HDR 10-bit") && !hdr_feature) {
        return None;
    }
    if !is_int_in(&result["quality"], 40, 100) || !is_int_in(&result["longest"], 0, 30000) {
        return None;
    }
    if !["sRGB", "Adobe RGB", "Display P3", "ProPhoto RGB"].contains(&result["color_space"].as_str()?) {
        return None;
    }
    if !result["keep_metadata"].is_boolean() {
        return None;
    }
    // Added in 0.5.53; earlier saved settings keep working with both enabled.
    for key in ["grayscale", "dither"] {
        let v = object.get(key).cloned().unwrap_or(Value::Bool(true));
        if !v.is_boolean() {
            return None;
        }
        result.insert(key.into(), v);
    }
    let naming = result["naming"].as_str()?;
    let name = pyformat::format(naming, &[("stem", Arg::Str("photo".into())), ("n", Arg::Int(1))]).ok()?;
    let platform = Platform::native();
    if name.trim().is_empty() || pathx::name(platform, &name) != name || name.chars().any(|c| "<>:\"/\\|?*".contains(c))
    {
        return None;
    }
    Some(result)
}

/// Port of `quick_export.copy_original`: creates a new original and XMP pair exclusively (an existing file is
/// an error) and cleans up only its own files on failure. `record` is the catalogue row (`path`, `info`, ...).
/// Returns `info['width']`, `info['height']` (0 when absent).
pub fn copy_original(record: &Value, destination: &Path) -> Result<(Value, Value)> {
    let platform = Platform::native();
    let destination_text = pathx::text(destination);
    let sidecar = pathx::with_suffix(platform, &destination_text, ".xmp");
    let mut created: Vec<String> = Vec::new();
    let outcome = (|| -> Result<()> {
        let temporary = tempfile::Builder::new().prefix("grainy-export-").tempdir()?;
        let settings = temporary.path().join("settings.xmp");
        write_sidecar(record, &settings)?;
        let original =
            record.get("path").and_then(Value::as_str).ok_or_else(|| Error::Type("KeyError: 'path'".into()))?;
        for (source, target) in
            [(original.to_owned(), destination_text.clone()), (pathx::text(&settings), sidecar.clone())]
        {
            let mut incoming = File::open(&source).map_err(|e| Error::os(&e, Path::new(&source), true))?;
            let mut outgoing = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map_err(|e| Error::os(&e, Path::new(&target), true))?;
            created.push(target.clone());
            io::copy(&mut incoming, &mut outgoing).map_err(|e| Error::os(&e, Path::new(&target), true))?;
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        for path in created.iter().rev() {
            let _ = fs::remove_file(path);
        }
        return Err(error);
    }
    let info = record.get("info").cloned().unwrap_or(json!({}));
    Ok((info.get("width").cloned().unwrap_or(json!(0)), info.get("height").cloned().unwrap_or(json!(0))))
}

/// Installed RAM in bytes (`render_cache.physical_memory`), `None` when it cannot be read.
pub fn physical_memory() -> Option<u64> {
    let Ok(mut system) = sysinfo::System::new() else { return None };
    system.refresh_memory();
    let total = system.total_memory();
    (total > 0).then_some(total)
}

/// Port of `quick_export.export_concurrency` with the machine given: photos exported at once. RAW decoding is
/// single-threaded, so a few in parallel raise throughput (measured in the oracle: 1 to 3 at a time cut a NEF
/// from 0.79 to 0.36 s). Bounded by CPU count and a quarter of RAM.
pub fn export_concurrency(records: &[Value], cpus: usize, memory: Option<u64>) -> Result<usize> {
    if records.len() < 2 || cpus < 8 {
        return Ok(1);
    }
    let mut pixels: i64 = 0;
    for r in records {
        let info = r.get("info").ok_or_else(|| Error::Type("KeyError: 'info'".into()))?;
        let side = |key: &str| -> Result<i64> {
            match info.get(key) {
                Some(v) if grainy_core::settings::truthy(v) => pyformat::py_int(v),
                _ => Ok(0),
            }
        };
        pixels = pixels.max(side("width")? * side("height")?);
    }
    if pixels == 0 {
        pixels = 24_000_000;
    }
    let budget = memory.filter(|m| *m > 0).unwrap_or(8 * 1024 * 1024 * 1024) / 4;
    let per_photo = (pixels as u64) * 3 * 4 * 8;
    let limit = (budget / per_photo.max(1)) as usize;
    Ok(1.max(4.min(cpus / 6).min(limit)))
}

/// [`export_concurrency`] for this machine (`os.cpu_count()` and the installed RAM).
pub fn export_concurrency_here(records: &[Value]) -> Result<usize> {
    let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    export_concurrency(records, cpus, physical_memory())
}
