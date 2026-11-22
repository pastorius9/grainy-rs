//! Port of `luma/preview_store.py` packet storage. Rendering and JPEG encoding belong to codecs/engine.
use crate::{integer, invalid, json, object, profiles, settings, string, Catalog, Result};
use grainy_core::Settings;
use serde_json::{json, Value};
use std::{
    fs,
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Port of `preview_store.VERSION`.
pub const VERSION: i64 = 4;
/// Port of `preview_store.signature`; directories, missing and inaccessible paths return None.
pub fn signature(path: impl AsRef<Path>) -> Option<Value> {
    let path = path.as_ref();
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let modified = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    let path = crate::path_text(&fs::canonicalize(path).ok()?);
    Some(json!({"path":path,"size":meta.len(),"mtime_ns":u64::try_from(modified).ok()?}))
}
/// Port of `preview_store.snapshot`; reads settings and preview token in one read transaction.
pub fn snapshot(directory: impl AsRef<Path>, id: i64, full: bool) -> Result<Option<Value>> {
    let directory = directory.as_ref();
    snapshot_from(directory, directory, id, full)
}

/// Read catalogue rows from a private database snapshot and preview signatures from the original library.
pub fn snapshot_from(database_folder: &Path, directory: &Path, id: i64, full: bool) -> Result<Option<Value>> {
    let c = Catalog::open_reader(database_folder)?;
    let columns = if full { "p.path,s.token,p.edits,p.metadata" } else { "p.path,s.token" };
    let Some(mut value) = c
        .rows(&format!("SELECT {columns} FROM photos p JOIN preview_state s ON s.photo_id=p.id WHERE p.id=?"), [id])?
        .pop()
    else {
        return Ok(None);
    };
    let path = string(&value["path"])?;
    let source = signature(path);
    let denied =
        source.is_none() && matches!(fs::metadata(path),Err(e) if e.kind()==std::io::ErrorKind::PermissionDenied);
    let offline = if source.is_none() { signature(directory.join(format!("previews/{id}.npz"))) } else { None };
    value["id"] = id.into();
    value["source"] = source.unwrap_or(Value::Null);
    value["denied"] = denied.into();
    value["offline"] = offline.unwrap_or(Value::Null);
    if full {
        value["settings"] = settings(&profiles::loads(&c.db, string(&value["edits"])?)?)?.0.into();
        value["info"] = serde_json::from_str(string(&value["metadata"])?)?;
    }
    Ok(Some(value))
}
/// Port of `preview_store.files`.
pub fn files(directory: impl AsRef<Path>, id: i64) -> (PathBuf, PathBuf) {
    let base = directory.as_ref().join("thumbnails");
    (base.join(format!("{id}.jpg")), base.join(format!("{id}.json")))
}
/// Port of `preview_store.inspect` result; the JPEG packet is returned for the UI/codec renderer.
pub struct Inspection {
    /// Original validated/decodeable JPEG from `preview_store.inspect` (even if its stamp is stale).
    pub image: Option<Vec<u8>>,
    /// JSON fields `rebuild`, `status`, `snapshot`, `color_space` from `preview_store.inspect`.
    pub state: Value,
}
fn check_jpeg(payload: &[u8]) -> Result<()> {
    if payload.len() > 4 * 1024 * 1024 {
        return Err(invalid("thumbnail too large"));
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(payload), image::ImageFormat::Jpeg);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    reader.decode().map_err(|e| invalid(format!("invalid thumbnail: {e}")))?;
    Ok(())
}
/// Port of `preview_store.inspect`, including useful stale/offline renders and stamp limits.
pub fn inspect(directory: impl AsRef<Path>, id: i64) -> Result<Inspection> {
    let current = snapshot(&directory, id, false)?;
    inspect_snapshot(directory, id, current)
}
/// Port of `preview_store.inspect` with an already captured snapshot for worker/test isolation.
pub fn inspect_snapshot(directory: impl AsRef<Path>, id: i64, current: Option<Value>) -> Result<Inspection> {
    let Some(current) = current else {
        return Ok(Inspection {
            image: None,
            state: json!({"rebuild":false,"status":"카탈로그에서 제거됨","snapshot":null}),
        });
    };
    let (jpeg, stamp) = files(&directory, id);
    let mut image = None;
    let mut valid = false;
    let mut color_space = "sRGB".to_owned();
    let attempt = (|| -> Result<()> {
        if fs::metadata(&jpeg)?.len() > 4 * 1024 * 1024 {
            return Err(invalid("thumbnail too large"));
        }
        let payload = fs::read(&jpeg)?;
        check_jpeg(&payload)?;
        image = Some(payload.clone());
        if fs::metadata(&stamp)?.len() > 8192 {
            return Err(invalid("thumbnail stamp too large"));
        }
        let meta: Value = serde_json::from_slice(&fs::read(&stamp)?)?;
        object(&meta)?;
        let stored_space = meta.get("color_space");
        color_space = stored_space.and_then(Value::as_str).unwrap_or("sRGB").into();
        if stored_space.is_some_and(|v| v.as_str().is_none())
            || !["sRGB", "ProPhoto RGB"].contains(&color_space.as_str())
        {
            color_space = "sRGB".into();
            return Err(invalid("invalid thumbnail color space"));
        }
        valid = json::equivalent(&meta["version"], &json!(VERSION))
            && meta["token"] == current["token"]
            && meta["path"] == current["path"]
            && meta["sha256"] == profiles::hash(&payload);
        if !current["source"].is_null() {
            valid = valid && json::equivalent(&meta["source"], &current["source"]);
        } else if !current["offline"].is_null() && meta["source"].is_null() {
            valid = valid && json::equivalent(&meta["offline"], &current["offline"]);
        }
        Ok(())
    })();
    if attempt.is_err() {
        valid = false;
    }
    let missing = current["source"].is_null();
    let rebuild = !valid && (!missing || !current["offline"].is_null());
    let denied = current["denied"] == true;
    let status = if denied {
        if valid {
            "접근 권한 필요"
        } else if rebuild {
            "갱신 중"
        } else if image.is_some() {
            "접근 권한 필요 · 이전 미리보기"
        } else {
            "접근 권한 필요"
        }
    } else if valid {
        if missing {
            "오프라인"
        } else {
            ""
        }
    } else if rebuild {
        "갱신 중"
    } else if image.is_some() {
        "원본 없음 · 이전 미리보기"
    } else {
        "원본 없음"
    };
    Ok(Inspection {
        image,
        state: json!({"rebuild":rebuild,"status":status,"snapshot":current,"color_space":color_space}),
    })
}
/// Port of `preview_store.replace_file`; Windows permission/sharing collisions retry for up to 95 ms.
pub fn replace_file(source: impl AsRef<Path>, target: impl AsRef<Path>) -> Result<()> {
    for attempt in 0..20 {
        match fs::rename(source.as_ref(), target.as_ref()) {
            Ok(()) => return Ok(()),
            Err(e)
                if attempt < 19
                    && (e.kind() == std::io::ErrorKind::PermissionDenied
                        || matches!(e.raw_os_error(), Some(32 | 33))) =>
            {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(invalid("replacement retry exhausted"))
}
fn cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|c| c.load(Ordering::Relaxed))
}
/// Port of `preview_store.publish` after JPEG encoding (RGB thumbnail, ICC profile, quality 88).
/// Validity is checked under an IMMEDIATE transaction before atomic replacement of each packet file.
pub fn publish(
    directory: impl AsRef<Path>,
    state: &Value,
    jpeg_payload: &[u8],
    cancel: Option<&AtomicBool>,
    color_space: &str,
) -> Result<bool> {
    if cancelled(cancel) {
        return Ok(false);
    }
    if !["sRGB", "ProPhoto RGB"].contains(&color_space) {
        return Err(invalid("unsupported thumbnail color space"));
    }
    check_jpeg(jpeg_payload)?;
    let directory = directory.as_ref();
    let id = integer(&state["id"])?;
    let path = string(&state["path"])?;
    string(&state["token"])?;
    let stamp = json!({"token":state["token"],"path":state["path"],"source":state["source"],"offline":state["offline"],"version":VERSION,"color_space":color_space,"sha256":profiles::hash(jpeg_payload)});
    let (jpeg, meta) = files(directory, id);
    fs::create_dir_all(directory.join("thumbnails"))?;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let unique = format!(
        "{}.{}.{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| invalid("clock before epoch"))?.as_nanos(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let temp = jpeg.with_file_name(format!("{id}.jpg.{unique}.tmp"));
    let temp_meta = meta.with_file_name(format!("{id}.json.{unique}.tmp"));
    let work = (|| -> Result<bool> {
        fs::write(&temp, jpeg_payload)?;
        fs::write(&temp_meta, json::dumps(&stamp, true, false, false))?;
        let db = rusqlite::Connection::open(directory.join("catalog.sqlite"))?;
        db.busy_timeout(Duration::from_secs(10))?;
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<bool> {
            let row = db
                .prepare("SELECT p.path,s.token FROM photos p JOIN preview_state s ON p.id=s.photo_id WHERE p.id=?")?
                .query_map([id], crate::row_value)?
                .next()
                .transpose()?;
            let Some(row) = row else { return Ok(false) };
            if row["token"] != state["token"] || row["path"] != state["path"] {
                return Ok(false);
            }
            if signature(path).unwrap_or(Value::Null) != state["source"] {
                return Ok(false);
            }
            if state["source"].is_null()
                && signature(directory.join(format!("previews/{id}.npz"))).unwrap_or(Value::Null) != state["offline"]
            {
                return Ok(false);
            }
            if cancelled(cancel) {
                return Ok(false);
            }
            replace_file(&temp, &jpeg)?;
            replace_file(&temp_meta, &meta)?;
            Ok(true)
        })();
        let rollback = db.execute_batch("ROLLBACK");
        let result = result?;
        rollback?;
        Ok(result)
    })();
    let _ = fs::remove_file(temp);
    let _ = fs::remove_file(temp_meta);
    work
}
/// Port of `preview_store.store_render` after JPEG encoding; rejects stale settings/source signatures.
pub fn store_render(
    directory: impl AsRef<Path>,
    id: i64,
    settings_value: &Settings,
    jpeg: &[u8],
    source: &Value,
    offline: &Value,
    color_space: &str,
) -> Result<bool> {
    let Some(state) = snapshot(&directory, id, true)? else { return Ok(false) };
    if !json::equivalent(&state["settings"], &Value::Object(settings_value.0.clone()))
        || state["source"] != *source
        || state["offline"] != *offline
    {
        return Ok(false);
    }
    publish(directory, &state, jpeg, None, color_space)
}

/// Stored array in `preview_store.load_offline` NPZ packets (C-order H×W×3).
#[derive(Clone, Debug, PartialEq)]
pub struct Pixels {
    /// Array dimensions stored in `pixels.npy` or `legacy_pixels.npy`.
    pub shape: [usize; 3],
    /// Values converted to float32 as in `load_offline`.
    pub values: Vec<f32>,
    /// Whether the packet stores float16 rather than float32.
    pub half: bool,
}
/// Port of the NPZ packet consumed by `preview_store.load_offline`.
#[derive(Clone, Debug, PartialEq)]
pub struct OfflinePacket {
    /// Main `pixels` array, including native camera pixels when present.
    pub pixels: Pixels,
    /// JSON `info` scalar, retaining RAW metadata and working space.
    pub info: Value,
    /// Optional legacy rendering for a native RAW packet.
    pub legacy_pixels: Option<Pixels>,
}
fn read_npy(data: &[u8]) -> Result<(String, Vec<usize>, &[u8])> {
    if data.get(..6) != Some(b"\x93NUMPY") {
        return Err(invalid("invalid NPY header"));
    }
    let version = *data.get(6).ok_or_else(|| invalid("truncated NPY"))?;
    let (start, len) = match version {
        1 => {
            let bytes = data.get(8..10).ok_or_else(|| invalid("truncated NPY"))?;
            (10, u16::from_le_bytes([bytes[0], bytes[1]]) as usize)
        }
        2 | 3 => {
            let bytes = data.get(8..12).ok_or_else(|| invalid("truncated NPY"))?;
            (12, u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize)
        }
        _ => return Err(invalid("unsupported NPY version")),
    };
    let end = start + len;
    let header = std::str::from_utf8(data.get(start..end).ok_or_else(|| invalid("truncated NPY header"))?)
        .map_err(|_| invalid("invalid NPY text"))?;
    if !header.contains("'fortran_order': False") {
        return Err(invalid("only C-order NPY packets supported"));
    }
    let descr = header
        .split("'descr':")
        .nth(1)
        .and_then(|s| s.trim().strip_prefix('\''))
        .and_then(|s| s.split('\'').next())
        .ok_or_else(|| invalid("missing NPY dtype"))?
        .to_owned();
    let shape = header
        .split("'shape':")
        .nth(1)
        .and_then(|s| s.split('(').nth(1))
        .and_then(|s| s.split(')').next())
        .ok_or_else(|| invalid("missing NPY shape"))?;
    let shape = shape
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<usize>().map_err(|_| invalid("invalid NPY shape")))
        .collect::<Result<Vec<_>>>()?;
    Ok((descr, shape, &data[end..]))
}
fn pixels(data: &[u8]) -> Result<Pixels> {
    let (dtype, shape, bytes) = read_npy(data)?;
    if shape.len() != 3 || shape[2] != 3 {
        return Err(invalid("offline pixels must be HxWx3"));
    }
    let half = match dtype.as_str() {
        "<f2" => true,
        "<f4" => false,
        _ => return Err(invalid("offline pixels must be little endian float16 or float32")),
    };
    let count =
        shape.iter().try_fold(1usize, |a, b| a.checked_mul(*b)).ok_or_else(|| invalid("pixel size overflow"))?;
    let size = if half { 2 } else { 4 };
    if count.checked_mul(size) != Some(bytes.len()) {
        return Err(invalid("pixel length mismatch"));
    }
    let values = if half {
        bytes.as_chunks::<2>().0.iter().map(|b| half::f16::from_bits(u16::from_le_bytes(*b)).to_f32()).collect()
    } else {
        bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
    };
    Ok(Pixels { shape: [shape[0], shape[1], shape[2]], values, half })
}
fn npy(dtype: &str, shape: &[usize], bytes: &[u8]) -> Vec<u8> {
    let dims = if shape.is_empty() {
        String::new()
    } else {
        format!("{},", shape.iter().map(usize::to_string).collect::<Vec<_>>().join(", "))
    };
    let mut header = format!("{{'descr': '{dtype}', 'fortran_order': False, 'shape': ({dims}), }}");
    let pad = (64 - (10 + header.len() + 1) % 64) % 64;
    header.push_str(&" ".repeat(pad));
    header.push('\n');
    let mut out = b"\x93NUMPY\x01\x00".to_vec();
    out.extend((header.len() as u16).to_le_bytes());
    out.extend(header.as_bytes());
    out.extend(bytes);
    out
}
fn encode_pixels(p: &Pixels) -> Result<Vec<u8>> {
    let count =
        p.shape.iter().try_fold(1usize, |a, b| a.checked_mul(*b)).ok_or_else(|| invalid("pixel size overflow"))?;
    if p.shape[2] != 3 || p.values.len() != count {
        return Err(invalid("invalid pixel dimensions"));
    }
    let bytes = if p.half {
        p.values.iter().flat_map(|v| half::f16::from_f32(*v).to_bits().to_le_bytes().to_vec()).collect::<Vec<_>>()
    } else {
        p.values.iter().flat_map(|v| v.to_le_bytes()).collect()
    };
    Ok(npy(if p.half { "<f2" } else { "<f4" }, &p.shape, &bytes))
}
/// Port of `preview_store.load_offline` packet reading, before camera-source/color conversion.
pub fn read_offline(path: impl AsRef<Path>) -> Result<OfflinePacket> {
    let file = fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| invalid(e.to_string()))?;
    let mut get = |name: &str| -> Result<Vec<u8>> {
        let mut f = zip.by_name(name).map_err(|e| invalid(e.to_string()))?;
        if f.size() > 256 * 1024 * 1024 {
            return Err(invalid("offline array too large"));
        }
        let mut data = Vec::new();
        f.read_to_end(&mut data)?;
        Ok(data)
    };
    let p = pixels(&get("pixels.npy")?)?;
    let data = get("info.npy")?;
    let (dtype, shape, bytes) = read_npy(&data)?;
    if !shape.is_empty() {
        return Err(invalid("offline info must be a scalar"));
    }
    let text = if let Some(n) = dtype.strip_prefix("<U") {
        let n = n.parse::<usize>().map_err(|_| invalid("invalid Unicode dtype"))?;
        if n.checked_mul(4) != Some(bytes.len()) {
            return Err(invalid("invalid Unicode scalar"));
        }
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| {
                char::from_u32(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                    .ok_or_else(|| invalid("invalid Unicode codepoint"))
            })
            .collect::<Result<String>>()?
            .trim_end_matches('\0')
            .to_owned()
    } else {
        return Err(invalid("offline info must be a Unicode scalar"));
    };
    let info = serde_json::from_str(&text)?;
    let legacy_pixels = match zip.by_name("legacy_pixels.npy") {
        Ok(mut file) => {
            if file.size() > 256 * 1024 * 1024 {
                return Err(invalid("offline array too large"));
            }
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Some(pixels(&bytes)?)
        }
        Err(zip::result::ZipError::FileNotFound) => None,
        Err(e) => return Err(invalid(e.to_string())),
    };
    Ok(OfflinePacket { pixels: p, info, legacy_pixels })
}
/// Writes the NPZ schema consumed by `preview_store.load_offline` (as `manager.py` does).
/// Image processing, RAW wrapping, and working-space conversion remain caller responsibilities.
pub fn write_offline(path: impl AsRef<Path>, packet: &OfflinePacket) -> Result<()> {
    let text = json::dumps(&packet.info, true, false, false);
    let info = npy(
        &format!("<U{}", text.chars().count()),
        &[],
        &text.chars().flat_map(|c| (c as u32).to_le_bytes()).collect::<Vec<_>>(),
    );
    let mut arrays = vec![("pixels.npy", encode_pixels(&packet.pixels)?), ("info.npy", info)];
    if let Some(p) = &packet.legacy_pixels {
        arrays.push(("legacy_pixels.npy", encode_pixels(p)?));
    }
    let path = path.as_ref();
    let mut stage = tempfile::NamedTempFile::new_in(path.parent().unwrap_or_else(|| Path::new(".")))?;
    let mut zip = zip::ZipWriter::new(stage.as_file_mut());
    for (name, data) in arrays {
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
        )
        .map_err(|e| invalid(e.to_string()))?;
        zip.write_all(&data)?;
    }
    zip.finish().map_err(|e| invalid(e.to_string()))?.sync_all()?;
    stage.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Port of `preview_store.load_offline` after unpacking: chooses legacy/native pixels and marks provenance.
/// A caller-supplied color-management adapter ports `colorio.convert`; native camera RGB is identified by
/// `info.raw_native` and carries the unmodified `info.raw_info` needed by `rawcolor.CameraSource`.
pub fn load_offline(
    directory: impl AsRef<Path>,
    id: i64,
    settings: &Settings,
    convert: impl FnOnce(Pixels, &str, &str) -> Result<Pixels>,
) -> Result<(Pixels, Value)> {
    let path = directory.as_ref().join(format!("previews/{id}.npz"));
    let fingerprint = signature(&path).unwrap_or(Value::Null);
    let packet = read_offline(&path)?;
    let mut source = packet.pixels;
    let mut info = packet.info;
    object(&info)?;
    let enabled = settings.str("raw_mode") != "legacy" || settings.bool("dcp_profile") || settings.bool("negative");
    if !enabled {
        if let Some(legacy) = packet.legacy_pixels {
            source = legacy;
            info["raw_native"] = false.into();
        }
    }
    if grainy_core::settings::truthy(&info["raw_native"]) {
        if info.get("raw_info").is_none() {
            return Err(invalid("native offline preview is missing raw_info"));
        }
        info["working_space"] = settings.str("working_space").into();
    } else if enabled {
        return Err(invalid("offline preview has no native sensor information"));
    } else {
        let old = info["working_space"].as_str().unwrap_or("sRGB");
        let new = settings.str("working_space");
        if old != new {
            source = convert(
                source,
                if old == "ProPhoto" { "Linear ProPhoto" } else { "Linear sRGB" },
                if new == "ProPhoto" { "Linear ProPhoto" } else { "Linear sRGB" },
            )?;
            info["working_space"] = new.into();
        }
    }
    source.half = false;
    info["offline_preview"] = true.into();
    info["offline_fingerprint"] = fingerprint;
    Ok((source, info))
}

/// Default on-disk thumbnail + offline preview cache limit (2 GiB).
pub const DEFAULT_CACHE_LIMIT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Summary returned by [`prune_cache`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PruneStats {
    pub removed_files: usize,
    pub freed_bytes: u64,
}

/// Removes orphaned `thumbnails/{id}.*` and `previews/{id}.*` cache entries (for deleted photos),
/// cleans stale `.tmp` files, and evicts the oldest remaining thumbnail entries (LRU by mtime) when
/// total thumbnail cache size exceeds `max_bytes`. Live offline Smart Previews (`previews/{id}.npz`)
/// are never evicted by thumbnail LRU so offline editing packets stay intact.
pub fn prune_cache(directory: impl AsRef<Path>, max_bytes: u64) -> Result<PruneStats> {
    let directory = directory.as_ref();
    // Open in standard read-only mode (not `immutable=1`) so live WAL rows from active writers are
    // always visible and newly imported photos are never mistaken for orphans.
    let db_path = directory.join("catalog.sqlite");
    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(2))?;
    let mut live_ids = std::collections::HashSet::<i64>::new();
    {
        let mut stmt = conn.prepare("SELECT id FROM photos")?;
        for id in stmt.query_map([], |row| row.get::<_, i64>(0))? {
            live_ids.insert(id?);
        }
    }
    drop(conn);

    let now = SystemTime::now();
    let mut stats = PruneStats::default();
    let mut groups: std::collections::HashMap<i64, (SystemTime, u64, Vec<PathBuf>)> =
        std::collections::HashMap::new();

    for (is_thumb, sub) in [(true, "thumbnails"), (false, "previews")] {
        let dir = directory.join(sub);
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let size = meta.len();
            let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.ends_with(".tmp") {
                if now.duration_since(mtime).unwrap_or_default() > Duration::from_secs(300)
                    && fs::remove_file(&path).is_ok()
                {
                    stats.removed_files += 1;
                    stats.freed_bytes += size;
                }
                continue;
            }
            let Some((stem, ext)) = name.rsplit_once('.') else { continue };
            if !matches!(ext, "jpg" | "json" | "npz") {
                continue;
            }
            let Ok(id) = stem.parse::<i64>() else { continue };
            if !live_ids.contains(&id) {
                if fs::remove_file(&path).is_ok() {
                    stats.removed_files += 1;
                    stats.freed_bytes += size;
                }
                continue;
            }
            // Only thumbnail cache entries participate in LRU eviction; keep live offline Smart Previews.
            if is_thumb {
                let slot = groups.entry(id).or_insert_with(|| (mtime, 0, Vec::new()));
                if mtime > slot.0 {
                    slot.0 = mtime;
                }
                slot.1 += size;
                slot.2.push(path);
            }
        }
    }

    let mut total: u64 = groups.values().map(|g| g.1).sum();
    if total > max_bytes {
        let mut ordered: Vec<(SystemTime, u64, Vec<PathBuf>)> = groups.into_values().collect();
        ordered.sort_by_key(|(mtime, _, _)| *mtime);
        for (_, _, paths) in ordered {
            if total <= max_bytes {
                break;
            }
            for path in paths {
                let bytes = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                if fs::remove_file(&path).is_ok() {
                    stats.removed_files += 1;
                    stats.freed_bytes += bytes;
                    total = total.saturating_sub(bytes);
                }
            }
        }
    }
    Ok(stats)
}

#[cfg(test)]
mod prune_tests {
    use super::*;

    #[test]
    fn prune_cache_removes_orphans_and_evicts_oldest_over_budget() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Catalog::new(dir.path()).unwrap();
        let p1 = dir.path().join("a.jpg");
        let p2 = dir.path().join("b.jpg");
        fs::write(&p1, b"a").unwrap();
        fs::write(&p2, b"b").unwrap();
        let id1 = catalog.add(p1.to_str().unwrap(), None, None, false).unwrap();
        let id2 = catalog.add(p2.to_str().unwrap(), None, None, false).unwrap();
        drop(catalog);

        let thumbs = dir.path().join("thumbnails");
        fs::create_dir_all(&thumbs).unwrap();
        fs::write(thumbs.join("9999.jpg"), vec![0u8; 400]).unwrap();
        fs::write(thumbs.join("9999.json"), b"{}").unwrap();
        fs::write(thumbs.join(format!("{id1}.jpg")), vec![1u8; 500]).unwrap();
        std::thread::sleep(Duration::from_millis(25));
        fs::write(thumbs.join(format!("{id2}.jpg")), vec![2u8; 500]).unwrap();

        let stats = prune_cache(dir.path(), 600).unwrap();
        assert!(!thumbs.join("9999.jpg").exists(), "orphan removed");
        assert!(!thumbs.join("9999.json").exists(), "orphan stamp removed");
        assert!(!thumbs.join(format!("{id1}.jpg")).exists(), "oldest entry evicted by LRU");
        assert!(thumbs.join(format!("{id2}.jpg")).exists(), "newest entry retained under budget");
        assert!(stats.removed_files >= 3 && stats.freed_bytes >= 900);
    }
}
