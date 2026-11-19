//! 메타데이터: the keywords of the open photo, the metadata box for one or many photos, the capture-time shift, the
//! EXIF refresh of the whole library, the keyword tree and 파일 위치 열기 (luma/app.py `save_keywords`, `show_file`;
//! luma/manager.py `metadata`, `shift_date`, `refresh_metadata`, `keyword_tree`).
//!
//! The catalogue work is in plain functions over a [`Catalog`] and photo ids ([`apply_form`], [`shift_dates`],
//! [`keyword_groups`], [`refresh_rows`], [`set_keywords`]): the commands, the dialogs and the tests call them.
//! Everything that edits needs a library that is open for writing (a read-only library and a session have no
//! catalogue rows to write).

use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, Duration, NaiveDateTime};
use grainy_catalog::rusqlite::params;
use grainy_catalog::Catalog;
use serde_json::{json, Map, Value};

use super::{has_photos, photo_open, writable_library, Command, Invocation};
use crate::context::{AppContext, Mode};
use crate::dialogs::keyword_tree::KeywordTree;
use crate::dialogs::metadata::{MetadataDialog, ShiftDialog};
use crate::panels::metadata::MetadataState;
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "metadata.edit", label: "메타데이터 편집", enabled: can_edit, run: edit },
    Command { id: "metadata.shift_date", label: "촬영 시각 일괄 이동", enabled: can_edit, run: shift_date },
    Command {
        id: "metadata.refresh", label: "전체 EXIF 메타데이터 새로 읽기", enabled: can_refresh, run: refresh
    },
    Command {
        id: "metadata.keyword_tree", label: "계층형 키워드 트리", enabled: has_catalog, run: keyword_tree
    },
    Command { id: "metadata.keywords", label: "키워드", enabled: can_keyword, run: keywords },
    Command { id: "metadata.show_file", label: "파일 위치 열기", enabled: photo_open, run: show_file },
];

/// The fields of the metadata box, in the oracle's order: (key in the photo's extras, label).
pub const FIELDS: [(&str, &str); 10] = [
    ("title", "제목"),
    ("caption", "설명"),
    ("creator", "촬영자"),
    ("copyright", "저작권"),
    ("location", "장소"),
    ("city", "도시"),
    ("country", "국가"),
    ("DateTimeOriginal", "촬영 시각 YYYY:MM:DD HH:MM:SS"),
    ("latitude", "위도"),
    ("longitude", "경도"),
];

/// The capture time's text format (`'%Y:%m:%d %H:%M:%S'`).
const STAMP: &str = "%Y:%m:%d %H:%M:%S";

/// `datetime.strptime(text, '%Y:%m:%d %H:%M:%S')`: Python's year has exactly four digits and runs from 1 to 9999.
pub(crate) fn parse_stamp(text: &str) -> Result<NaiveDateTime, String> {
    let four = text.len() > 4 && text.as_bytes()[..4].iter().all(u8::is_ascii_digit) && text.as_bytes()[4] == b':';
    NaiveDateTime::parse_from_str(text, STAMP)
        .ok()
        .filter(|d| four && (1..=9999).contains(&d.year()))
        .ok_or_else(|| format!("time data {} does not match format '%Y:%m:%d %H:%M:%S'", python_repr(text)))
}

fn can_edit(cx: &AppContext) -> bool {
    writable_library(cx) && has_photos(cx)
}

fn can_keyword(cx: &AppContext) -> bool {
    writable_library(cx) && photo_open(cx)
}

fn can_refresh(cx: &AppContext) -> bool {
    writable_library(cx) && !cx.jobs.running("metadata_refresh")
}

fn has_catalog(cx: &AppContext) -> bool {
    cx.doc.metadata_catalog().is_some()
}

/// The ids of the selected photos (`selected_ids`).
pub fn selected_ids(cx: &AppContext) -> Vec<i64> {
    cx.doc.selected_indices().iter().filter_map(|i| cx.doc.photos().get(*i)).map(|p| p.id as i64).collect()
}

fn failed(cx: &mut AppContext, message: &str) {
    cx.status = tr_with("처리하지 못했습니다: {0}", &[&tr(message)]);
}

/// Tells the panel and the list that catalogue rows changed (`load_controls()` and `refresh_lists()`).
pub fn changed(cx: &mut AppContext) {
    cx.state::<MetadataState>().revision += 1;
    cx.doc.request_refresh(false);
}

// ---- Python's text of a value ---------------------------------------------------------------------------------

/// Python's `repr` of a float (`repr(0.1)`, `1e-05`, `1e+16`, `100.0`).
fn python_float(v: f64) -> String {
    if v.is_nan() {
        return "nan".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "inf" } else { "-inf" }.into();
    }
    let sign = if v.is_sign_negative() { "-" } else { "" };
    if v == 0.0 {
        return format!("{sign}0.0");
    }
    // `{:e}` gives the shortest digits that read back exactly, as `repr` does.
    let text = format!("{:e}", v.abs());
    let (mantissa, exponent) = text.split_once('e').unwrap_or((&text, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    if (-4..16).contains(&exponent) {
        if exponent >= 0 {
            let whole = exponent as usize + 1;
            if digits.len() <= whole {
                format!("{sign}{digits}{}.0", "0".repeat(whole - digits.len()))
            } else {
                format!("{sign}{}.{}", &digits[..whole], &digits[whole..])
            }
        } else {
            format!("{sign}0.{}{digits}", "0".repeat((-exponent - 1) as usize))
        }
    } else {
        let head = &digits[..1];
        let tail = &digits[1..];
        let point = if tail.is_empty() { String::new() } else { format!(".{tail}") };
        format!("{sign}{head}{point}e{}{:02}", if exponent < 0 { '-' } else { '+' }, exponent.abs())
    }
}

/// Python's `repr` of a string.
pub(crate) fn python_repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') { '"' } else { '\'' };
    let mut out = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => out.push_str(&format!("\\x{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

fn python_repr_value(value: &Value) -> String {
    match value {
        Value::String(s) => python_repr(s),
        Value::Array(items) => format!("[{}]", items.iter().map(python_repr_value).collect::<Vec<_>>().join(", ")),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(k, v)| format!("{}: {}", python_repr(k), python_repr_value(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        other => python_str(other),
    }
}

/// Python's `str(value)` of a JSON value as `json.loads` makes it (`str(original.get(key, ''))`,
/// `f'{k}: {v}'`): `None`, `True`, `100.0`, `['a', 'b']`.
pub fn python_str(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => i.to_string(),
            (None, Some(u)) => u.to_string(),
            _ => python_float(n.as_f64().unwrap_or(0.0)),
        },
        Value::String(s) => s.clone(),
        other => python_repr_value(other),
    }
}

/// The FILE section's text (`file_info`): the path, a blank line, then `key: value` for every item of the
/// photo's stored metadata except the two fingerprints.
pub fn info_text(path: &str, info: &Value) -> String {
    let lines: Vec<String> = info
        .as_object()
        .map(|map| {
            map.iter()
                .filter(|(k, _)| *k != "file_fingerprint" && *k != "offline_fingerprint")
                .map(|(k, v)| format!("{k}: {}", python_str(v)))
                .collect()
        })
        .unwrap_or_default();
    format!("{path}\n\n{}", lines.join("\n"))
}

// ---- the metadata box -----------------------------------------------------------------------------------------

/// The metadata box's values: what the first selected photo had and what the fields say now.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    /// The ten fields of [`FIELDS`] as the first photo had them (`str(original.get(key, ''))`).
    pub original: Vec<String>,
    pub original_keywords: String,
    pub values: Vec<String>,
    pub keywords: String,
}

impl Form {
    /// The box for `ids` (the first photo's values).
    pub fn load(catalog: &Catalog, ids: &[i64]) -> Option<Self> {
        let photo = catalog.photo(*ids.first()?).ok()??;
        let extras = photo["user_metadata"].as_object().cloned().unwrap_or_default();
        let original: Vec<String> =
            FIELDS.iter().map(|(key, _)| extras.get(*key).map(python_str).unwrap_or_default()).collect();
        let keywords = photo["keywords"].as_str().unwrap_or("").to_owned();
        Some(Self { values: original.clone(), original, original_keywords: keywords.clone(), keywords })
    }

    /// Sets a field by its key.
    pub fn set(&mut self, key: &str, text: &str) {
        if let Some(index) = FIELDS.iter().position(|(k, _)| *k == key) {
            self.values[index] = text.to_owned();
        }
    }
}

/// Port of `LibraryManager.metadata` after 저장: only the fields that differ from the first photo's go to every
/// selected photo (`set_user_metadata`), the keywords when they differ. The GPS ranges and the capture time are
/// checked first; the message of a refusal is the oracle's, and nothing is written then.
pub fn apply_form(catalog: &Catalog, ids: &[i64], form: &Form) -> Result<(), String> {
    let mut changes = Map::new();
    for (index, (key, _)) in FIELDS.iter().enumerate() {
        if form.values[index] != form.original[index] {
            changes.insert((*key).to_owned(), Value::String(form.values[index].clone()));
        }
    }
    for (key, limit) in [("latitude", 90.0), ("longitude", 180.0)] {
        let Some(text) = changes.get(key).and_then(Value::as_str).filter(|t| !t.is_empty()) else { continue };
        let number: f64 =
            text.trim().parse().map_err(|_| format!("could not convert string to float: {}", python_repr(text)))?;
        if !(-limit..=limit).contains(&number) {
            return Err("GPS 좌표 범위를 확인하세요.".into());
        }
    }
    if let Some(text) = changes.get("DateTimeOriginal").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        parse_stamp(text)?;
    }
    for id in ids {
        catalog.set_user_metadata(*id, &changes).map_err(|e| e.to_string())?;
        if form.keywords != form.original_keywords {
            set_keywords(catalog, *id, &form.keywords)?;
        }
    }
    Ok(())
}

/// Port of `save_keywords` for one photo: the keyword text as typed.
pub fn set_keywords(catalog: &Catalog, id: i64, text: &str) -> Result<(), String> {
    let mut fields = Map::new();
    fields.insert("keywords".into(), Value::String(text.to_owned()));
    catalog.update(id, &fields).map_err(|e| e.to_string())
}

// ---- the capture-time shift -----------------------------------------------------------------------------------

/// Port of `LibraryManager.shift_date`: each photo's capture time (the entered one, else the file's `date`)
/// moved by `minutes`; photos without a time are left out. All times are read before any is written, so a time
/// that cannot be read changes nothing. Returns how many photos changed.
pub fn shift_dates(catalog: &Catalog, ids: &[i64], minutes: i64) -> Result<usize, String> {
    let mut updates = Vec::new();
    for id in ids {
        let Some(photo) = catalog.photo(*id).map_err(|e| e.to_string())? else { continue };
        let entered = &photo["user_metadata"]["DateTimeOriginal"];
        let stamp = if grainy_core::settings::truthy(entered) { entered } else { &photo["info"]["date"] };
        if !grainy_core::settings::truthy(stamp) {
            continue;
        }
        let text = python_str(stamp);
        let parsed = parse_stamp(&text)?;
        let moved = Duration::try_minutes(minutes)
            .and_then(|d| parsed.checked_add_signed(d))
            .filter(|d| (1..=9999).contains(&d.year()))
            .ok_or_else(|| "date value out of range".to_owned())?;
        updates.push((*id, moved.format(STAMP).to_string()));
    }
    for (id, stamp) in &updates {
        let mut values = Map::new();
        values.insert("DateTimeOriginal".into(), Value::String(stamp.clone()));
        catalog.set_user_metadata(*id, &values).map_err(|e| e.to_string())?;
    }
    Ok(updates.len())
}

// ---- the keyword tree -----------------------------------------------------------------------------------------

/// The keyword tree's nodes of `keyword_tree`: every `|`-joined path with the number of photos under it, sorted
/// by path.
pub fn keyword_groups(catalog: &Catalog) -> Result<Vec<(String, usize)>, String> {
    let mut groups: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for photo in catalog.photos().map_err(|e| e.to_string())? {
        let id = photo["id"].as_i64().unwrap_or(0);
        for keyword in photo["keywords"].as_str().unwrap_or("").split(',') {
            let parts: Vec<&str> = keyword.trim().split('|').map(str::trim).filter(|k| !k.is_empty()).collect();
            for end in 1..=parts.len() {
                groups.entry(parts[..end].join("|")).or_default().insert(id);
            }
        }
    }
    Ok(groups.into_iter().map(|(path, ids)| (path, ids.len())).collect())
}

/// What a double click on a keyword does (`select` of `keyword_tree`): the list shows the photos with that
/// keyword (the filter rules become `{'keywords': path}`, no collection, no folder) in the grid.
pub fn filter_keyword(cx: &mut AppContext, path: &str) {
    let mut query = cx.doc.query();
    query.rules = json!({ "keywords": path });
    query.collection = None;
    query.folder = None;
    if let Err(message) = cx.doc.set_query(query) {
        failed(cx, &message);
        return;
    }
    cx.set_mode(Mode::Library);
}

// ---- the EXIF refresh -----------------------------------------------------------------------------------------

/// The new stored metadata of every photo the file's EXIF could be read for, and how many could not
/// (`refresh_metadata`'s `work`): the stored dictionary with the file's values over it. Videos are skipped.
pub fn refresh_rows(catalog: &Catalog, cancelled: &dyn Fn() -> bool) -> Result<(Vec<(i64, Value)>, usize), String> {
    let photos = catalog.photos().map_err(|e| e.to_string())?;
    let mut results = Vec::new();
    let mut failures = 0;
    for photo in photos {
        if cancelled() {
            break;
        }
        let path = photo["path"].as_str().unwrap_or("");
        let ext = std::path::Path::new(path).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase()));
        if ext.is_some_and(|e| [".mp4", ".mov", ".avi", ".mkv", ".m4v"].contains(&e.as_str())) {
            continue;
        }
        let stored = serde_json::from_str::<Value>(photo["metadata"].as_str().unwrap_or("{}"));
        let read = grainy_codecs::read_metadata(path);
        match (stored, read) {
            (Ok(Value::Object(mut info)), Ok(read)) => {
                info.extend(read);
                results.push((photo["id"].as_i64().unwrap_or(0), Value::Object(info)));
            }
            _ => failures += 1,
        }
    }
    Ok((results, failures))
}

/// Port of `refresh_metadata`'s `done`: the rows written in one transaction. Returns how many.
pub fn store_rows(catalog: &Catalog, rows: &[(i64, Value)]) -> Result<usize, String> {
    catalog
        .transaction(|| {
            for (id, info) in rows {
                catalog.connection().execute(
                    "UPDATE photos SET metadata=? WHERE id=?",
                    params![grainy_catalog::json::dumps(info, true, false, false), id],
                )?;
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
    Ok(rows.len())
}

// ---- commands -------------------------------------------------------------------------------------------------

/// 메타데이터 편집: opens the box for the selected photos.
fn edit(cx: &mut AppContext, _: &Invocation) {
    let ids = selected_ids(cx);
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    let Some(form) = Form::load(catalog, &ids) else { return };
    cx.open_dialog(Box::new(MetadataDialog::new(ids, form)));
}

/// 촬영 시각 일괄 이동: asks for the minutes.
fn shift_date(cx: &mut AppContext, invocation: &Invocation) {
    // A value runs it without the box (menus never give one; scripts and tests do).
    if let Some(minutes) = invocation.value.as_ref().and_then(Value::as_i64) {
        shift_now(cx, minutes);
        return;
    }
    cx.open_dialog(Box::new(ShiftDialog::new()));
}

/// The shift of the selected photos, with the oracle's status line.
pub fn shift_now(cx: &mut AppContext, minutes: i64) {
    let ids = selected_ids(cx);
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    match shift_dates(catalog, &ids, minutes) {
        Ok(count) => {
            cx.status = tr_with("{0}장 촬영 시각을 변경했습니다.", &[&count.to_string()]);
            changed(cx);
        }
        Err(message) => failed(cx, &message),
    }
}

/// The metadata box's 저장.
pub fn save_form(cx: &mut AppContext, ids: &[i64], form: &Form) {
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    match apply_form(catalog, ids, form) {
        Ok(()) => changed(cx),
        Err(message) => failed(cx, &message),
    }
}

/// 키워드 field of the panel (`save_keywords`): the text goes to the open photo.
fn keywords(cx: &mut AppContext, invocation: &Invocation) {
    let Some(text) = invocation.value.as_ref().and_then(Value::as_str) else { return };
    let id = cx.doc.current_id() as i64;
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    match set_keywords(catalog, id, text) {
        Ok(()) => changed(cx),
        Err(message) => failed(cx, &message),
    }
}

/// 전체 EXIF 메타데이터 새로 읽기: a job reads every file's EXIF, the window stores the rows.
fn refresh(cx: &mut AppContext, _: &Invocation) {
    let Some(folder) = cx.doc.library_folder() else { return };
    let source = folder.clone();
    cx.jobs.spawn(
        "metadata_refresh",
        tr("전체 EXIF 메타데이터 새로 읽기"),
        move |handle| {
            let catalog = Catalog::open_reader(&source).map_err(|e| e.to_string())?;
            handle.progress(0, 1, tr("전체 EXIF 메타데이터 새로 읽기"));
            refresh_rows(&catalog, &|| handle.cancelled())
        },
        move |cx: &mut AppContext, result: Result<(Vec<(i64, Value)>, usize), String>| {
            // Another library may be open by now.
            if cx.doc.library_folder().as_deref() != Some(folder.as_path()) {
                return;
            }
            let (rows, failures) = match result {
                Ok(done) => done,
                Err(message) => return failed(cx, &message),
            };
            let Some(catalog) = cx.doc.metadata_catalog() else { return };
            match store_rows(catalog, &rows) {
                Ok(count) => {
                    changed(cx);
                    cx.status = tr_with(
                        "메타데이터 {0}장 갱신 · 읽을 수 없음 {1}장",
                        &[&count.to_string(), &failures.to_string()],
                    );
                }
                Err(message) => failed(cx, &message),
            }
        },
    );
}

/// 계층형 키워드 트리: the box over the library's keywords.
fn keyword_tree(cx: &mut AppContext, _: &Invocation) {
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    match keyword_groups(catalog) {
        Ok(groups) => {
            cx.close_dialog("keyword_tree");
            cx.open_dialog(Box::new(KeywordTree::new(groups)));
        }
        Err(message) => failed(cx, &message),
    }
}

/// 파일 위치 열기 (`show_file`): the folder of the open photo in the file manager.
fn show_file(cx: &mut AppContext, _: &Invocation) {
    let Some(photo) = cx.doc.current() else { return };
    let Some(folder) = photo.path.parent() else { return };
    let folder = folder.to_string_lossy().into_owned();
    cx.run_with("library.open_folder", &Invocation::value(folder));
}
