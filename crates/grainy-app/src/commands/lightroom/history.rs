//! Port of luma/adobe_history.py `transfer` (and `decode_row` with the warnings kept in the order they were first
//! counted): Lightroom's develop history and snapshots of one photo, translated state by state into the library's
//! `history` and `snapshots` rows, and the last 80 translated history states into the photo's undo list
//! (`undo:{id}`). grainy-adobe has the pure parts (`adobe_time`, `decode_row`, `convert`) but not `transfer`,
//! which writes the catalogue: it lives here, privately (see the package report, "Requests").

use std::collections::VecDeque;

use grainy_adobe::develop::{convert, from_lua, Converted};
use grainy_adobe::history::{adobe_time, HISTORY_PREFIX};
use grainy_adobe::{ErrorKind, Lua};
use grainy_catalog::rusqlite::{params, Connection};
use grainy_core::settings::{equivalent, truthy};
use grainy_core::Settings;
use serde_json::{Map, Value};

use super::import::{isoformat, py_str, Clock, Failure};

/// The oracle's `Counter` of warnings: counts by key, in the order the keys were first counted.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Warnings(pub Vec<(String, i64)>);

impl Warnings {
    /// `warnings[key] += n`.
    pub fn add(&mut self, key: &str, n: i64) {
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some((_, count)) => *count += n,
            None => self.0.push((key.to_owned(), n)),
        }
    }

    /// `warnings.pop(key, None)`.
    pub fn remove(&mut self, key: &str) {
        self.0.retain(|(k, _)| k != key);
    }

    /// `dict(warnings)`.
    pub fn to_value(&self) -> Value {
        Value::Object(self.0.iter().map(|(k, n)| (k.clone(), Value::from(*n))).collect())
    }
}

/// Port of `adobe_history.decode_row(row, is_raw, orientation, warnings)`: the translated row, or `None` (counted)
/// for a row that cannot be read or has nothing Grainy can use.
pub fn decode_row(
    row: &Value,
    is_raw: bool,
    orientation: Option<&Lua>,
    warnings: &mut Warnings,
) -> Result<Option<Converted>, Failure> {
    let text = match row.get("text") {
        Some(value) if truthy(value) => value.clone(),
        _ => Value::String(String::new()),
    };
    let converted = match &text {
        Value::String(text) => from_lua(text).and_then(|values| convert(&values, None, is_raw, orientation)),
        // `from_lua` of a value that is not text: `DataError`.
        _ => Err(grainy_adobe::Error::data("저장된 데이터의 크기 또는 문자 형식을 확인할 수 없습니다.")),
    };
    let result = match converted {
        Ok(result) => result,
        Err(e) if matches!(e.kind, ErrorKind::Data | ErrorKind::Value | ErrorKind::Type | ErrorKind::Overflow) => {
            warnings.add("develop_unreadable", 1);
            return Ok(None);
        }
        Err(e) => return Err(Failure::Error(format!("{}: {}", e.kind.python_name(), e.message))),
    };
    for (key, _) in &result.omitted.0 {
        warnings.add(&format!("develop_field:{key}"), 1);
    }
    if result.mapped.is_empty() {
        warnings.add("develop_empty", 1);
        return Ok(None);
    }
    Ok(Some(result))
}

/// The keys of `engine.DEFAULTS` in the oracle's order (`list(normalized({}))`): `json.dumps` of settings writes
/// them in this order. grainy-core's defaults are sorted, so the order is kept here (see "Requests").
const ORDER: [&str; 106] = [
    "exposure",
    "contrast",
    "highlights",
    "shadows",
    "whites",
    "blacks",
    "temperature",
    "tint",
    "saturation",
    "vibrance",
    "clarity",
    "sharpen",
    "vignette",
    "grain",
    "vignette_midpoint",
    "vignette_roundness",
    "vignette_feather",
    "vignette_highlights",
    "grain_size",
    "grain_roughness",
    "rotation",
    "straighten",
    "flip",
    "crop",
    "curve",
    "hsl",
    "monochrome",
    "video_color",
    "video_time",
    "hdr",
    "hdr_limit",
    "hdr_brightness",
    "sdr_exposure",
    "sdr_compression",
    "photo_filter",
    "photo_filter_enabled",
    "photo_filter_density",
    "photo_filter_luminosity",
    "tone_version",
    "detail_version",
    "rgb_curves",
    "parametric",
    "mixer_mode",
    "kelvin",
    "wb_reference",
    "kelvin_enabled",
    "grading",
    "grading_balance",
    "calibration",
    "point_colors",
    "texture",
    "dehaze",
    "noise_luma",
    "noise_color",
    "sharpen_radius",
    "sharpen_detail",
    "sharpen_mask",
    "distortion",
    "distortion_k2",
    "distortion_k3",
    "lens_vignette",
    "ca_red",
    "ca_blue",
    "defringe",
    "perspective_v",
    "perspective_h",
    "aspect_scale",
    "shift_x",
    "shift_y",
    "transform_scale",
    "guides",
    "lens_profile",
    "camera_matrix",
    "lensfun",
    "lensfun_enabled",
    "lensfun_distortion",
    "lensfun_tca",
    "lensfun_vignette",
    "lensfun_scale",
    "upright",
    "upright_crop",
    "masks",
    "retouch",
    "working_space",
    "raw_mode",
    "raw_kelvin",
    "raw_tint",
    "raw_neutral",
    "dcp_profile",
    "dcp_huesat",
    "dcp_look",
    "dcp_tone",
    "dcp_exposure",
    "negative",
    "negative_base",
    "negative_peak",
    "negative_range",
    "negative_density",
    "film_look",
    "film_strength",
    "tone_mapping",
    "drt_contrast",
    "drt_toe",
    "drt_shoulder",
    "drt_white",
    "panels_off",
];

/// The settings in the oracle's dict order (`normalized` builds them on `DEFAULTS`): the default keys first, in
/// their order, then any others as they come.
fn ordered(settings: &Map<String, Value>) -> Map<String, Value> {
    let mut out = Map::new();
    for key in ORDER {
        if let Some(value) = settings.get(key) {
            out.insert(key.to_owned(), value.clone());
        }
    }
    for (key, value) in settings {
        if !out.contains_key(key) {
            out.insert(key.clone(), value.clone());
        }
    }
    out
}

fn dumps(value: &Value) -> String {
    // `json.dumps(value, ensure_ascii=False)`: the settings in their own order, with Python's separators.
    grainy_catalog::json::dumps(value, false, false, false)
}

/// Port of `adobe_history.transfer(db, image_id, photo_id, current, *, is_raw, orientation, warnings,
/// cancel_check)`: returns `(history, snapshots)` counts. Rows stream; only the last 80 successfully translated
/// history states enter Undo, and a row that cannot be translated breaks Undo continuity.
#[allow(clippy::too_many_arguments)]
pub fn transfer(
    db: &Connection,
    image_id: &str,
    photo_id: i64,
    current: Option<&Settings>,
    is_raw: bool,
    orientation: Option<&Lua>,
    warnings: &mut Warnings,
    clock: &mut dyn Clock,
    cancel_check: &dyn Fn() -> Result<(), Failure>,
) -> Result<(i64, i64), Failure> {
    let (mut history, mut snapshots) = (0i64, 0i64);
    let mut undo: VecDeque<Map<String, Value>> = VecDeque::new();
    for (kind, is_history) in
        [("Adobe_libraryImageDevelopHistoryStep", true), ("Adobe_libraryImageDevelopSnapshot", false)]
    {
        let mut cursor = db.prepare(
            "SELECT payload FROM lr_stage WHERE kind=? AND image=?
            ORDER BY CAST(json_extract(payload,'$.dateCreated') AS REAL),CAST(id AS INTEGER),id",
        )?;
        let mut rows = cursor.query(params![kind, image_id])?;
        while let Some(packed) = rows.next()? {
            cancel_check()?;
            let text: String = packed.get(0)?;
            let row: Value = serde_json::from_str(&text).map_err(|e| Failure::value(e.to_string()))?;
            let Some(converted) = decode_row(&row, is_raw, orientation, warnings)? else {
                if is_history {
                    undo.clear();
                }
                continue;
            };
            let settings = ordered(&converted.settings.0);
            let name_value = row.get("name").unwrap_or(&Value::Null);
            let mut name = if truthy(name_value) { py_str(name_value) } else { "이름 없는 상태".to_owned() };
            if !converted.omitted.is_empty() {
                name += " · 일부 항목 변환";
            }
            let timestamp = match adobe_time(row.get("dateCreated").unwrap_or(&Value::Null)) {
                Some(t) => t,
                None => isoformat(clock.utc()),
            };
            let payload = dumps(&Value::Object(settings.clone()));
            if is_history {
                db.execute(
                    "INSERT INTO history(photo_id,label,edits,created) VALUES(?,?,?,?)",
                    params![photo_id, format!("{HISTORY_PREFIX}{name}"), payload, timestamp],
                )?;
                let same = undo
                    .back()
                    .is_some_and(|last| equivalent(&Value::Object(last.clone()), &Value::Object(settings.clone())));
                if !same {
                    undo.push_back(settings);
                    if undo.len() > 80 {
                        undo.pop_front();
                    }
                }
                history += 1;
            } else {
                db.execute(
                    "INSERT INTO snapshots(photo_id,name,edits,created) VALUES(?,?,?,?)",
                    params![photo_id, format!("Lightroom · {name}"), payload, timestamp],
                )?;
                snapshots += 1;
            }
        }
    }
    // Lightroom history entries describe the state after an operation. Grainy's undo list contains earlier
    // states, so a final entry equal to the current one is left out.
    if let (Some(last), Some(current)) = (undo.back(), current) {
        if equivalent(&Value::Object(last.clone()), &Value::Object(current.0.clone())) {
            undo.pop_back();
        }
    }
    if !undo.is_empty() && current.is_some() {
        let mut journal = Map::new();
        journal.insert("undo".into(), Value::Array(undo.into_iter().map(Value::Object).collect()));
        journal.insert("redo".into(), Value::Array(Vec::new()));
        db.execute(
            "INSERT OR REPLACE INTO preferences VALUES(?,?)",
            params![format!("undo:{photo_id}"), dumps(&Value::Object(journal))],
        )?;
    }
    Ok((history, snapshots))
}
