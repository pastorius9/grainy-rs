//! Dust detection for many photos (여러 사진의 먼지 감지): port of luma/dust_batch_dialog.py `DustBatchDialog`, with
//! the parts of luma/dust_store.py (`DustReviewStore`, `read_work`) and luma/dust_batch.py (`capture_record`,
//! `verify_source`, `apply_reviewed`, `restore`) that grainy-dust leaves to the application: the review store's
//! transactions and the batch's apply and undo on grainy-service's command batches.
//!
//! [`DustBatch`] is the dialog without a window (shared with the jobs' callbacks and the review dialogs it opens);
//! its methods are the oracle's of the same name. Detection runs one photo at a time on the job service; the work
//! is saved in the catalogue (`dust_sessions`, `dust_entries`) after every step, so it survives closing the window.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use eframe::egui::{self, Color32, Vec2};
use grainy_catalog::rusqlite::{params, OptionalExtension};
use grainy_catalog::{profiles, Catalog};
use serde_json::{json, Value};

use super::confirm::Confirm;
use super::dust::{self, thousands, BatchReview, DustReview, MAX_CANDIDATES};
use super::Dialog;
use crate::context::AppContext;
use crate::text::{tr, tr_with};
use crate::theme as t;
use crate::widgets::{self, Button, Kind};

/// `BATCH_KEY`: the preference of the last batch apply (`command_batch` group).
pub const BATCH_KEY: &str = "undo:dust_batch";
/// `dust_store.VERSION`.
pub const VERSION: i64 = 1;
/// `STATES`: (state, title), in the oracle's order.
pub const STATES: [(&str, &str); 6] = [
    ("pending", "검출 대기"),
    ("scanning", "검출 중"),
    ("ready", "검토 대기"),
    ("reviewed", "검토 완료"),
    ("failed", "확인 필요"),
    ("applied", "적용 완료"),
];
/// `dust_store.STATES`: the states a saved entry may have.
const STORED: [&str; 5] = ["pending", "ready", "reviewed", "failed", "applied"];
const VIDEO: [&str; 5] = ["mp4", "mov", "avi", "mkv", "m4v"];
const CANCELLED: &str = "\u{0}cancelled";

fn video(path: &str) -> bool {
    Path::new(path).extension().and_then(|e| e.to_str()).is_some_and(|e| VIDEO.contains(&e.to_lowercase().as_str()))
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// The session ids of new works: `uuid4().hex` (random), or the ids a test gives in order.
#[derive(Default)]
pub struct SessionIds {
    /// Ids handed out first, front to back.
    pub queue: std::collections::VecDeque<String>,
}

fn new_session_id(cx: &mut AppContext, catalog_id: impl FnOnce() -> String) -> String {
    cx.state::<SessionIds>().queue.pop_front().unwrap_or_else(catalog_id)
}

// ---- dust_batch.py ------------------------------------------------------------------------------------------

/// Port of luma/dust_batch.py `capture_record`: the photo's path, settings and file signature.
pub fn capture_record(catalog: &Catalog, id: i64) -> Result<Value, String> {
    let row = catalog.photo(id).map_err(err)?.ok_or_else(|| "카탈로그에서 사진을 찾을 수 없습니다.".to_owned())?;
    let path = row["path"].as_str().unwrap_or_default().to_owned();
    if video(&path) {
        return Err("먼지 감지는 사진에서 사용할 수 있습니다.".into());
    }
    let fingerprint = grainy_catalog::preview::signature(&path).unwrap_or(Value::Null);
    Ok(json!({"id": id, "path": path, "settings": row["settings"], "fingerprint": fingerprint}))
}

/// Port of luma/dust_batch.py `verify_source`: the photo's path and settings (`command_batch.verify`) and its file.
pub fn verify_source(catalog: &Catalog, record: &Value) -> Result<(), String> {
    grainy_service::batch::verify(catalog, std::slice::from_ref(record)).map_err(err)?;
    let path = record["path"].as_str().unwrap_or_default();
    let same = !record["fingerprint"].is_null()
        && grainy_catalog::preview::signature(path)
            .is_some_and(|s| grainy_core::settings::equivalent(&s, &record["fingerprint"]));
    if !same {
        return Err("원본 파일이 없거나 바뀌었습니다. 해당 사진을 다시 검출해 주세요.".into());
    }
    Ok(())
}

type Hook<'a> = &'a mut dyn FnMut(&[i64]) -> grainy_service::Result<()>;

/// Port of luma/dust_batch.py `apply_reviewed`: every reviewed entry with chosen spots gets its own operation
/// appended to its own settings, all in one command batch (`먼지 일괄 제거`, kept under [`BATCH_KEY`]).
pub fn apply_reviewed(catalog: &Catalog, entries: &[Value], after_apply: Hook<'_>) -> Result<Vec<i64>, String> {
    let mut records = Vec::new();
    let mut values = Vec::new();
    for entry in entries {
        let spots = entry["operation"]["spots"].as_array().is_some_and(|s| !s.is_empty());
        if entry["state"] != "reviewed" || !spots {
            continue;
        }
        let record = &entry["record"];
        verify_source(catalog, record)?;
        let mut value = record["settings"].clone();
        if let Some(list) = value.get_mut("retouch").and_then(Value::as_array_mut) {
            list.push(entry["operation"].clone());
        }
        records.push(record.clone());
        values.push(value);
    }
    let options = grainy_service::batch::ApplyOptions {
        label: "먼지 일괄 제거".into(),
        batch_key: BATCH_KEY.into(),
        ..Default::default()
    };
    grainy_service::batch::apply(catalog, &records, &values, &options, Some(after_apply)).map_err(err)
}

/// Port of luma/dust_batch.py `restore`: undo (or redo) the last batch apply.
pub fn restore(catalog: &Catalog, redo: bool, after_restore: Hook<'_>) -> Result<Vec<i64>, String> {
    grainy_service::batch::restore(catalog, redo, BATCH_KEY, Some(after_restore), None).map_err(err)
}

// ---- dust_store.py ------------------------------------------------------------------------------------------

/// An error of the store: the oracle's text.
#[derive(Debug)]
struct StoreError(String);

impl From<grainy_catalog::rusqlite::Error> for StoreError {
    fn from(e: grainy_catalog::rusqlite::Error) -> Self {
        Self(e.to_string())
    }
}

impl From<String> for StoreError {
    fn from(e: String) -> Self {
        Self(e)
    }
}

type StoreResult<T> = Result<T, StoreError>;

fn bad(text: &str) -> StoreError {
    StoreError(text.into())
}

fn in_transaction(catalog: &Catalog) -> bool {
    !catalog.connection().is_autocommit()
}

/// `DustReviewStore.transaction`.
fn transaction<T>(catalog: &Catalog, nested: bool, f: impl FnOnce() -> StoreResult<T>) -> StoreResult<T> {
    if nested {
        if !in_transaction(catalog) {
            return Err(bad("먼지 검토 저장 트랜잭션이 없습니다."));
        }
        return f();
    }
    if in_transaction(catalog) {
        return Err(bad("다른 저장 작업이 진행 중입니다."));
    }
    grainy_catalog::transaction::run(catalog.connection(), true, f)
}

fn options_valid(options: &Value) -> bool {
    serde_json::from_value::<grainy_dust::Options>(options.clone()).is_ok_and(|o| dust::validate(&o).is_ok())
}

fn unit(value: &Value, zero: bool) -> bool {
    value.as_f64().is_some_and(|v| v.is_finite() && (0.0..=1.0).contains(&v) && (zero || v != 0.0))
}

/// Port of `DustReviewStore.validate_spots`.
fn validate_spots(spots: &Value) -> StoreResult<()> {
    let Some(spots) = spots.as_array() else {
        return Err(bad("저장된 먼지 좌표가 올바르지 않습니다."));
    };
    for spot in spots {
        for key in ["x", "y", "rx", "ry"] {
            if !unit(&spot[key], key == "x" || key == "y") {
                return Err(bad("저장된 먼지 좌표가 올바르지 않습니다."));
            }
        }
        let polarity = spot["polarity"].as_str().unwrap_or("");
        let scale = spot.get("scale").map_or(Some(100.0), Value::as_f64);
        if !["dark", "bright", "manual"].contains(&polarity) || !scale.is_some_and(|s| (25.0..=250.0).contains(&s)) {
            return Err(bad("저장된 먼지 선택이 올바르지 않습니다."));
        }
        if spot.get("parts").is_some() {
            grainy_dust::shapes::ellipses(spot, (100, 100)).map_err(|e| StoreError(e.to_string()))?;
        }
    }
    Ok(())
}

/// Port of `DustReviewStore.validate`.
fn validate(value: &Value) -> StoreResult<()> {
    let state = value.get("state").and_then(Value::as_str).unwrap_or("");
    if !value.is_object() || !STORED.contains(&state) {
        return Err(bad("저장된 검토 상태가 올바르지 않습니다."));
    }
    let record = &value["record"];
    if !record["id"].is_i64() || !record["path"].is_string() || !record["settings"].is_object() {
        return Err(bad("저장된 사진 정보가 올바르지 않습니다."));
    }
    if ["ready", "reviewed", "applied"].contains(&state) {
        if !options_valid(&value["options"]) {
            return Err(bad("저장된 먼지 후보가 올바르지 않습니다."));
        }
        let result = &value["result"];
        let spots = result["spots"].as_array();
        let size = result["width"].as_f64().unwrap_or(0.0).min(result["height"].as_f64().unwrap_or(0.0));
        if spots.is_none_or(|s| s.len() > MAX_CANDIDATES) || size < 1.0 {
            return Err(bad("저장된 먼지 후보가 올바르지 않습니다."));
        }
        validate_spots(&result["spots"])?;
    }
    if ["reviewed", "applied"].contains(&state) {
        let operation = &value["operation"];
        let review = &value["review"];
        if operation["type"] != "dust" || operation["spots"].as_array().is_none_or(|s| s.len() > 6000) {
            return Err(bad("저장된 제거 선택이 올바르지 않습니다."));
        }
        let length = |key: &str| review[key].as_array().map_or(usize::MAX / 2, Vec::len);
        if !review.is_object() || length("spots") + length("pending") > 6000 {
            return Err(bad("저장된 검토 목록이 올바르지 않습니다."));
        }
        let listed = length("spots");
        let indices = review["checked"].as_array().cloned().unwrap_or_default();
        if indices.iter().any(|i| !i.is_i64() || !(0..listed as i64).contains(&i.as_i64().unwrap_or(-1))) {
            return Err(bad("저장된 후보 선택이 올바르지 않습니다."));
        }
        let unique: BTreeSet<i64> = indices.iter().filter_map(Value::as_i64).collect();
        if unique.len() != indices.len() {
            return Err(bad("저장된 후보 선택이 중복됐습니다."));
        }
        validate_spots(&review["spots"])?;
        validate_spots(&review["pending"])?;
        let chosen: Vec<Value> =
            indices.iter().filter_map(Value::as_u64).map(|i| review["spots"][i as usize].clone()).collect();
        if operation["spots"].as_array() != Some(&chosen) {
            return Err(bad("검토한 후보와 제거 선택이 일치하지 않습니다."));
        }
        let method = operation.get("repair_method").and_then(Value::as_str).unwrap_or("smooth");
        let review_method = review.get("repair_method").and_then(Value::as_str).unwrap_or("smooth");
        if !["smooth", "texture"].contains(&method) || review_method != method {
            return Err(bad("저장된 복구 방식이 올바르지 않습니다."));
        }
    }
    Ok(())
}

/// Port of `DustReviewStore.encode`: the entry (a scanning one as pending), validated, as compressed JSON.
fn encode(catalog: &Catalog, entry: &Value) -> StoreResult<(Vec<u8>, String)> {
    let mut value = entry.clone();
    if value["state"] == "scanning" {
        value["state"] = json!("pending");
    }
    validate(&value)?;
    let packed = profiles::pack(catalog.connection(), &value, false).map_err(|e| StoreError(e.to_string()))?;
    let payload =
        grainy_dust::store::pack(&packed).map_err(|_| bad("이 사진의 검토 내용이 저장 한도를 넘었습니다."))?;
    let state = value["state"].as_str().unwrap_or_default().to_owned();
    Ok((payload, state))
}

/// Port of `DustReviewStore.decode`.
fn decode(catalog: &Catalog, payload: &[u8]) -> StoreResult<Value> {
    let text = grainy_dust::store::unpack(payload).map_err(|_| bad("저장된 검토 파일이 손상됐습니다."))?;
    let value = profiles::loads(catalog.connection(), &text).map_err(|e| StoreError(e.to_string()))?;
    validate(&value)?;
    Ok(value)
}

/// Port of `DustReviewStore.sessions`: the saved works, newest first.
pub fn sessions(catalog: &Catalog) -> Result<Vec<Value>, String> {
    catalog
        .rows(
            "SELECT s.id,s.scope,s.snapshot,s.target_key,s.created,s.updated,
            COUNT(e.ordinal) AS total,SUM(e.state='reviewed') AS reviewed,SUM(e.state='applied') AS applied
            FROM dust_sessions s LEFT JOIN dust_entries e ON e.session_id=s.id
            GROUP BY s.id ORDER BY s.updated DESC,s.id",
            [],
        )
        .map_err(err)
}

/// Port of `DustReviewStore.create`: a new work with every entry; returns its id and revision 0.
fn create(
    catalog: &Catalog,
    ident: &str,
    snapshot: &Value,
    scope: &str,
    options: &Value,
    entries: &[Value],
    current: i64,
) -> StoreResult<(String, i64)> {
    if !options_valid(options) {
        return Err(bad("먼지 크기는 2–160 px 범위에서 최소 ≤ 최대여야 합니다."));
    }
    let now = catalog.now();
    let key = grainy_dust::store::target_key(snapshot, scope).map_err(|e| StoreError(e.to_string()))?;
    transaction(catalog, false, || {
        let db = catalog.connection();
        db.execute(
            "INSERT INTO dust_sessions VALUES(?,?,?,?,?,?,?,?,?,?)",
            params![
                ident,
                VERSION,
                key,
                scope,
                grainy_catalog::json::dumps(snapshot, false, false, false),
                grainy_catalog::json::dumps(options, true, false, false),
                now,
                now,
                current,
                0
            ],
        )?;
        for (index, entry) in entries.iter().enumerate() {
            let (payload, state) = encode(catalog, entry)?;
            let record = &entry["record"];
            db.execute(
                "INSERT INTO dust_entries VALUES(?,?,?,?,?,?)",
                params![ident, index as i64, record["id"].as_i64(), record["path"].as_str(), state, payload],
            )?;
        }
        Ok(())
    })?;
    Ok((ident.to_owned(), 0))
}

/// Port of `DustReviewStore.save`: the changed entries and the current row; returns the next revision.
fn save(
    catalog: &Catalog,
    ident: &str,
    revision: i64,
    changes: &[(usize, Value)],
    current: i64,
    nested: bool,
) -> StoreResult<i64> {
    transaction(catalog, nested, || {
        let db = catalog.connection();
        let changed = db.execute(
            "UPDATE dust_sessions SET revision=revision+1,updated=?,current_index=? WHERE id=? AND revision=? AND version=?",
            params![catalog.now(), current, ident, revision, VERSION],
        )?;
        if changed != 1 {
            return Err(bad("다른 창에서 검토 작업이 바뀌었습니다. 저장한 작업을 다시 열어 주세요."));
        }
        for (index, entry) in changes {
            let (payload, state) = encode(catalog, entry)?;
            let record = &entry["record"];
            let changed = db.execute(
                "UPDATE dust_entries SET photo_id=?,path=?,state=?,payload=? WHERE session_id=? AND ordinal=?",
                params![record["id"].as_i64(), record["path"].as_str(), state, payload, ident, *index as i64],
            )?;
            if changed != 1 {
                return Err(bad("저장된 검토 목록이 바뀌었습니다. 다시 열어 주세요."));
            }
        }
        Ok(())
    })?;
    Ok(revision + 1)
}

/// What [`load`] reads: the header (with its snapshot and options parsed), the entries, the damaged rows.
pub type Loaded = (Value, Vec<Value>, Vec<usize>);

/// Port of `DustReviewStore.load`: a saved work; damaged entries (and, with `verify_sources`, entries whose photo
/// or file changed) come back as 확인 필요.
fn load(
    catalog: &Catalog,
    ident: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
    verify_sources: bool,
) -> StoreResult<Loaded> {
    let stop = || if cancel.load(Ordering::Relaxed) { Err(StoreError(CANCELLED.into())) } else { Ok(()) };
    stop()?;
    let db = catalog.connection();
    let header =
        catalog.rows("SELECT * FROM dust_sessions WHERE id=?", [ident]).map_err(|e| StoreError(e.to_string()))?;
    let mut header = match header.into_iter().next() {
        Some(h) if h["version"].as_i64() == Some(VERSION) => h,
        _ => return Err(bad("이 버전에서 읽을 수 없는 검토 작업입니다.")),
    };
    let parse = |v: &Value| serde_json::from_str::<Value>(v.as_str().unwrap_or("")).unwrap_or(Value::Null);
    header["snapshot"] = parse(&header["snapshot"]);
    header["options"] = parse(&header["options"]);
    let snapshot = &header["snapshot"];
    let scope = header["scope"].as_str().unwrap_or("");
    if !["folder", "selection"].contains(&scope) || !snapshot.is_object() || !snapshot["scopes"].is_object() {
        return Err(bad("저장된 작업 대상이 올바르지 않습니다."));
    }
    if !snapshot["folder"].is_null() && !snapshot["folder"].is_string() {
        return Err(bad("저장된 폴더가 올바르지 않습니다."));
    }
    for s in ["folder", "selection"] {
        if snapshot["scopes"][s].as_array().is_none_or(|ids| ids.iter().any(|i| !i.is_i64())) {
            return Err(bad("저장된 사진 목록이 올바르지 않습니다."));
        }
    }
    if grainy_dust::store::target_key(snapshot, scope).ok().as_deref() != header["target_key"].as_str() {
        return Err(bad("저장된 작업 대상이 일치하지 않습니다."));
    }
    if !options_valid(&header["options"]) {
        return Err(bad("먼지 크기는 2–160 px 범위에서 최소 ≤ 최대여야 합니다."));
    }
    let total: i64 = db.query_row("SELECT COUNT(*) FROM dust_entries WHERE session_id=?", [ident], |r| r.get(0))?;
    let mut statement =
        db.prepare("SELECT ordinal,photo_id,path,state,payload FROM dust_entries WHERE session_id=? ORDER BY ordinal")?;
    let rows: Vec<(i64, i64, String, String, Vec<u8>)> = statement
        .query_map([ident], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
        .collect::<Result<_, _>>()?;
    let mut entries: Vec<Value> = Vec::new();
    let mut damaged = Vec::new();
    for (ordinal, photo_id, path, state, payload) in rows {
        stop()?;
        if ordinal != entries.len() as i64 {
            return Err(bad("저장된 검토 목록 순서가 올바르지 않습니다."));
        }
        let decoded = decode(catalog, &payload).and_then(|entry| {
            if entry["record"]["id"].as_i64() != Some(photo_id)
                || entry["record"]["path"].as_str() != Some(&path)
                || entry["state"].as_str() != Some(&state)
            {
                return Err(bad("검토 정보가 일치하지 않습니다."));
            }
            Ok(entry)
        });
        let mut entry = match decoded {
            Ok(entry) => entry,
            Err(_) => {
                let record = capture_record(catalog, photo_id).unwrap_or_else(|_| {
                    json!({"id": photo_id, "path": path, "settings": Value::Object(grainy_core::defaults().0),
                           "fingerprint": null})
                });
                damaged.push(ordinal as usize);
                json!({"record": record, "state": "failed", "result": null, "options": null, "operation": null,
                       "review": null, "error": "저장된 검토 내용이 손상됐습니다. 이 사진을 다시 검출해 주세요."})
            }
        };
        let state = entry["state"].as_str().unwrap_or("").to_owned();
        if verify_sources && ["pending", "ready", "reviewed"].contains(&state.as_str()) {
            if let Err(error) = verify_source(catalog, &entry["record"]) {
                entry["state"] = json!("failed");
                entry["error"] = json!(error);
                entry["operation"] = Value::Null;
                entry["review"] = Value::Null;
                damaged.push(ordinal as usize);
            }
        }
        entries.push(entry);
        progress(entries.len(), total as usize);
    }
    stop()?;
    Ok((header, entries, damaged))
}

/// Port of `DustReviewStore.delete`.
fn delete(catalog: &Catalog, ident: &str, revision: i64) -> StoreResult<()> {
    transaction(catalog, false, || {
        let db = catalog.connection();
        if db.execute("DELETE FROM dust_sessions WHERE id=? AND revision=?", params![ident, revision])? != 1 {
            return Err(bad("다른 창에서 검토 작업이 바뀌었습니다. 목록을 다시 열어 주세요."));
        }
        db.execute("DELETE FROM dust_entries WHERE session_id=?", [ident])?;
        Ok(())
    })
}

/// Port of luma/dust_store.py `read_work`: a saved work read on its own read-only connection.
pub fn read_work(
    directory: &Path,
    ident: &str,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<Option<Loaded>, String> {
    let catalog = Catalog::open_reader(directory).map_err(err)?;
    match load(&catalog, ident, cancel, progress, true) {
        Ok(loaded) => Ok(Some(loaded)),
        Err(StoreError(e)) if e == CANCELLED => Ok(None),
        Err(StoreError(e)) => Err(e),
    }
}

fn random_id(catalog: &Catalog) -> String {
    catalog
        .connection()
        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get::<_, String>(0))
        .optional()
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            format!(
                "{:032x}",
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
            )
        })
}

// ---- the dialog without a window ---------------------------------------------------------------------------

fn pending_entry(record: Value) -> Value {
    json!({"record": record, "state": "pending", "result": null, "options": null, "operation": null, "review": null,
           "error": ""})
}

fn commit_open_photo(cx: &mut AppContext) {
    cx.doc.edit_mut().commit("보정 변경");
    if let Err(message) = cx.doc.flush() {
        cx.set_status(tr_with("처리하지 못했습니다: {0}", &[&message]));
    }
}

/// `selected_ids()`: the selection in list order, or the open photo.
fn selected_ids(cx: &AppContext) -> Vec<i64> {
    let photos = cx.doc.photos();
    let ids: Vec<i64> = cx.doc.selected_indices().iter().filter_map(|i| photos.get(*i)).map(|p| p.id as i64).collect();
    if ids.is_empty() && cx.doc.current_id() != 0 {
        vec![cx.doc.current_id() as i64]
    } else {
        ids
    }
}

/// Port of `DustBatchDialog`'s state and methods.
pub struct DustBatch {
    pub closed: bool,
    pub busy: bool,
    pub running: bool,
    cancel: Arc<AtomicBool>,
    pub entries: Vec<Value>,
    active_index: i64,
    scan_indices: Vec<usize>,
    pub snapshot: Value,
    current_snapshot: Value,
    pub session_id: Option<String>,
    pub revision: i64,
    pub dirty: BTreeSet<usize>,
    pub save_error: String,
    pub loading: bool,
    load_token: u64,
    // The controls.
    pub scope: String,
    pub sensitivity: i32,
    pub minimum: i32,
    pub maximum: i32,
    pub polarity: String,
    pub detailed: bool,
    pub soft: bool,
    pub suppress_grain: bool,
    pub reduce_patterns: bool,
    pub scratches: bool,
    pub scratch_width: i32,
    /// The list's current row.
    pub current: i64,
    pub status: String,
    pub scope_note: String,
    /// The progress bar: (minimum, maximum, value); maximum 0 is a busy bar.
    pub progress: (usize, usize, usize),
    progress_shared: Arc<Mutex<Option<(usize, usize)>>>,
    /// The saved works (`work_selector`): (text, session id, tool tip).
    pub works: Vec<(String, Option<String>, String)>,
    /// The chosen saved work.
    pub work: usize,
    /// A question to ask (the window opens it).
    question: Option<Question>,
}

/// The shared batch dialog.
pub type Shared = Rc<RefCell<DustBatch>>;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Question {
    Forget,
    DiscardOnClose,
}

impl DustBatch {
    /// Port of `DustBatchDialog.__init__` up to its list (the saved work of the same target opens with
    /// [`open`]).
    pub fn new(cx: &mut AppContext) -> Result<Self, String> {
        let folder = cx.doc.query().folder;
        let snapshot = {
            let catalog = cx.doc.metadata_catalog().ok_or_else(|| "라이브러리에서만 할 수 있습니다.".to_owned())?;
            let current = cx.doc.current_id();
            grainy_dust::batch::capture_scope_ids(
                catalog,
                (current != 0).then_some(current as i64),
                &selected_ids(cx),
                folder.as_deref(),
            )
            .map_err(err)?
        };
        let initial = dust::initial_options(cx.doc.preference("dust_detection_options").as_ref());
        let mut batch = Self {
            closed: false,
            busy: false,
            running: false,
            cancel: Arc::new(AtomicBool::new(false)),
            entries: Vec::new(),
            active_index: -1,
            scan_indices: Vec::new(),
            current_snapshot: snapshot.clone(),
            snapshot,
            session_id: None,
            revision: 0,
            dirty: BTreeSet::new(),
            save_error: String::new(),
            loading: false,
            load_token: 0,
            scope: if folder.is_some() { "folder" } else { "selection" }.into(),
            sensitivity: initial.sensitivity,
            minimum: initial.minimum,
            maximum: initial.maximum,
            polarity: initial.polarity.clone(),
            detailed: initial.detailed,
            soft: initial.soft,
            suppress_grain: initial.suppress_grain,
            reduce_patterns: initial.reduce_patterns,
            scratches: initial.scratches,
            scratch_width: initial.scratch_width,
            current: -1,
            status: tr("대상을 확인하고 검출 시작을 누르세요."),
            scope_note: String::new(),
            progress: (0, 1, 0),
            progress_shared: Arc::new(Mutex::new(None)),
            works: Vec::new(),
            work: 0,
            question: None,
        };
        batch.reset_entries(cx);
        Ok(batch)
    }

    /// `options()`.
    pub fn options(&self) -> grainy_dust::Options {
        grainy_dust::Options {
            sensitivity: self.sensitivity,
            minimum: self.minimum,
            maximum: self.maximum,
            polarity: self.polarity.clone(),
            detailed: self.detailed,
            reduce_patterns: self.reduce_patterns,
            soft: self.soft,
            suppress_grain: self.suppress_grain,
            scratches: self.scratches,
            scratch_width: self.scratch_width,
        }
    }

    /// Whether the controls are locked (`busy or running or save_error`).
    pub fn locked(&self) -> bool {
        self.busy || self.running || !self.save_error.is_empty()
    }

    fn entry(&self) -> Option<&Value> {
        usize::try_from(self.current).ok().and_then(|i| self.entries.get(i))
    }

    /// The four columns of row `index` and its two tool tips (`update_row`).
    pub fn row(&self, index: usize) -> [String; 6] {
        let entry = &self.entries[index];
        let path = entry["record"]["path"].as_str().unwrap_or_default();
        let name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let state = entry["state"].as_str().unwrap_or("");
        let title = STATES.iter().find(|(k, _)| *k == state).map_or("", |(_, t)| *t);
        let result = &entry["result"];
        let found = if result.is_object() {
            let total = thousands(result["total"].as_u64().unwrap_or(0) as usize);
            if result["truncated"].as_bool().unwrap_or(false) {
                format!("{total} · 일부 표시")
            } else {
                total
            }
        } else {
            "—".into()
        };
        let chosen = if entry["operation"].is_null() {
            "—".into()
        } else {
            entry["operation"]["spots"].as_array().map_or(0, Vec::len).to_string()
        };
        [name, title.into(), found, chosen, path.into(), entry["error"].as_str().unwrap_or("").into()]
    }

    /// The colour of row `index`'s state.
    pub fn state_colour(&self, index: usize) -> Color32 {
        match self.entries[index]["state"].as_str().unwrap_or("") {
            "ready" | "failed" => Color32::from_rgb(0xe9, 0xc4, 0x8b),
            "reviewed" | "applied" => Color32::from_rgb(0xa6, 0xd1, 0xba),
            _ => Color32::from_rgb(0xa7, 0xb1, 0xbc),
        }
    }

    fn count(&self, state: &str) -> usize {
        self.entries.iter().filter(|e| e["state"] == state).count()
    }

    fn changed(&self) -> Vec<&Value> {
        self.entries
            .iter()
            .filter(|e| e["state"] == "reviewed" && e["operation"]["spots"].as_array().is_some_and(|s| !s.is_empty()))
            .collect()
    }

    /// `scan_button.text()`.
    pub fn scan_text(&self) -> String {
        tr_with("남은 {0}장 검출", &[&thousands(self.count("pending"))])
    }

    /// `stop_button.text()`.
    pub fn stop_text(&self) -> String {
        if self.loading {
            tr("불러오기 중단")
        } else {
            tr("검출 중단")
        }
    }

    /// `apply_button.text()`.
    pub fn apply_text(&self) -> String {
        let changed = self.changed();
        let spots: usize = changed.iter().map(|e| e["operation"]["spots"].as_array().map_or(0, Vec::len)).sum();
        tr_with("검토한 {0}장 · 먼지 {1}개 적용", &[&changed.len().to_string(), &thousands(spots)])
    }

    /// `summary.text()`.
    pub fn summary(&self) -> String {
        let parts: Vec<String> = STATES
            .iter()
            .filter(|(k, _)| self.count(k) > 0)
            .map(|(k, title)| format!("{title} {}장", thousands(self.count(k))))
            .collect();
        if parts.is_empty() {
            "대상 사진이 없습니다.".into()
        } else {
            parts.join(" · ")
        }
    }

    /// `save_note.text()`.
    pub fn save_note(&self) -> String {
        if !self.save_error.is_empty() {
            tr("검토 저장 실패 · 창을 열어 둔 채 다시 시도하세요: ") + &self.save_error
        } else if self.session_id.is_some() {
            tr("검토 자동 저장됨 · 원본 파일 변경 없음")
        } else {
            tr("검출을 시작하면 작업을 자동 저장합니다.")
        }
    }

    /// The enabled state of each control, by the oracle's name (`update_controls`).
    pub fn enabled(&self, cx: &AppContext, name: &str) -> bool {
        let locked = self.locked();
        let entry = self.entry();
        match name {
            "work_selector" | "new_button" | "scope" | "sensitivity" | "minimum" | "maximum" | "polarity"
            | "detailed" | "scratches" => !locked,
            "forget_button" => !locked && self.session_id.is_some(),
            "save_button" => !self.busy,
            "scratch_width" => !locked && self.scratches,
            "reduce_patterns" => !locked && (self.detailed || self.scratches),
            "soft" => !locked && self.detailed,
            "suppress_grain" => !locked && self.detailed && self.soft,
            "scan_button" => !locked && self.count("pending") > 0,
            "stop_button" => locked && !self.cancel.load(Ordering::Relaxed),
            "retry_button" => !locked && entry.is_some(),
            "review_button" => !locked && entry.is_some_and(|e| e["state"] == "ready" || e["state"] == "reviewed"),
            "next_button" => !locked && self.count("ready") > 0,
            "apply_button" => !locked && !self.changed().is_empty(),
            "undo_button" | "redo_button" => {
                let wanted = if name == "undo_button" { "applied" } else { "undone" };
                !locked
                    && cx
                        .doc
                        .preference(BATCH_KEY)
                        .is_some_and(|g| g.get("state").and_then(Value::as_str) == Some(wanted))
            }
            _ => true,
        }
    }

    /// Port of `update_controls`' one stateful line: the current entry's error shows in the status line.
    pub fn update_controls(&mut self) {
        if self.closed {
            return;
        }
        if !self.locked() {
            if let Some(error) = self.entry().and_then(|e| e["error"].as_str()).filter(|e| !e.is_empty()) {
                self.status = error.to_owned();
            }
        }
    }

    /// The list's current row changes (`currentItemChanged` → `update_controls`).
    pub fn set_current(&mut self, index: i64) {
        self.current = index;
        self.update_controls();
    }

    /// Port of `reset_entries`: the target's photos, pending; a new, unsaved work.
    pub fn reset_entries(&mut self, cx: &mut AppContext) {
        if self.busy {
            return;
        }
        self.session_id = None;
        self.revision = 0;
        self.dirty.clear();
        self.save_error.clear();
        self.entries.clear();
        self.current = -1;
        let ids: Vec<i64> = self.snapshot["scopes"][&self.scope]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_i64).collect())
            .unwrap_or_default();
        if let Some(catalog) = cx.doc.metadata_catalog() {
            for id in ids {
                let Ok(Some(row)) = catalog.photo(id) else { continue };
                if video(row["path"].as_str().unwrap_or_default()) {
                    continue;
                }
                if let Ok(record) = capture_record(catalog, id) {
                    self.entries.push(pending_entry(record));
                }
            }
        }
        let mut note = tr_with("{0}장 · 영상 제외", &[&thousands(self.entries.len())]);
        if self.scope == "folder" {
            let folder = self.snapshot["folder"].as_str().filter(|f| !f.is_empty()).unwrap_or("폴더 없음");
            note.push_str(&tr_with(" · 하위 폴더 제외 · 필터에 가려진 사진 포함\n{0}", &[folder]));
        }
        self.scope_note = note;
        if !self.entries.is_empty() {
            self.current = 0;
        }
        self.progress = (0, self.entries.len().max(1), 0);
        self.status = tr("대상을 확인하고 검출 시작을 누르세요.");
        self.update_controls();
        self.refresh_saved(cx);
    }

    /// Port of `scope_changed`.
    pub fn scope_changed(&mut self, cx: &mut AppContext, scope: &str) {
        self.scope = scope.to_owned();
        self.snapshot = self.current_snapshot.clone();
        self.reset_entries(cx);
    }

    /// Port of `refresh_saved`: the saved works of the catalogue.
    pub fn refresh_saved(&mut self, cx: &AppContext) {
        let mut works = vec![(tr("새 작업 · 아직 검출 전"), None, String::new())];
        let saved = cx.doc.metadata_catalog().map(sessions).and_then(Result::ok).unwrap_or_default();
        for s in saved {
            let snapshot: Option<Value> = s["snapshot"].as_str().and_then(|t| serde_json::from_str(t).ok());
            let folder = snapshot.as_ref().and_then(|s| s["folder"].as_str()).map(str::to_owned);
            let date = s["updated"].as_str().and_then(|u| chrono::DateTime::parse_from_rfc3339(u).ok()).map(|d| {
                // Local time (`astimezone()`); UTC while the render clock is frozen, so a test's picture
                // does not depend on the machine's time zone.
                if crate::context::FROZEN_TIMING.load(Ordering::Relaxed) {
                    d.with_timezone(&chrono::Utc).format("%m/%d %H:%M").to_string()
                } else {
                    d.with_timezone(&chrono::Local).format("%m/%d %H:%M").to_string()
                }
            });
            let (title, date) = match (snapshot.is_some(), date) {
                (true, Some(date)) => {
                    let title = match (&folder, s["scope"].as_str()) {
                        (Some(f), Some("folder")) if !f.is_empty() => {
                            Path::new(f).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                        }
                        _ => "선택한 사진".into(),
                    };
                    (title, date)
                }
                _ => ("저장한 검토".into(), String::new()),
            };
            let count = |key: &str| s[key].as_i64().unwrap_or(0).to_string();
            let text = tr_with(
                "{0} · {1}장 · 검토 {2} · 적용 {3} · {4}",
                &[
                    &title,
                    &thousands(s["total"].as_u64().unwrap_or(0) as usize),
                    &count("reviewed"),
                    &count("applied"),
                    &date,
                ],
            );
            works.push((text, s["id"].as_str().map(str::to_owned), folder.unwrap_or_default()));
        }
        self.work = works.iter().position(|w| w.1 == self.session_id).unwrap_or(0);
        self.works = works;
    }

    /// Port of `new_work`: the current target again, as a new work.
    pub fn new_work(&mut self, cx: &mut AppContext) {
        if self.busy || self.running || !self.save_error.is_empty() {
            return;
        }
        let folder = cx.doc.query().folder;
        let current = cx.doc.current_id();
        let selected = selected_ids(cx);
        if let Some(catalog) = cx.doc.metadata_catalog() {
            if let Ok(snapshot) = grainy_dust::batch::capture_scope_ids(
                catalog,
                (current != 0).then_some(current as i64),
                &selected,
                folder.as_deref(),
            ) {
                self.current_snapshot = snapshot;
            }
        }
        self.snapshot = self.current_snapshot.clone();
        self.scope = if folder.is_some() { "folder" } else { "selection" }.into();
        self.reset_entries(cx);
    }

    fn current_index(&self) -> i64 {
        self.current
    }

    /// Port of `persist`: the dirty entries (and `indices`) saved; a new work is created when `create`.
    pub fn persist(&mut self, cx: &mut AppContext, indices: &[usize], make: bool) -> bool {
        self.dirty.extend(indices.iter().copied());
        if self.session_id.is_none() && !make {
            return true;
        }
        let ident = match &self.session_id {
            Some(_) => None,
            None => {
                let fallback = cx.doc.metadata_catalog().map(random_id).unwrap_or_default();
                Some(new_session_id(cx, || fallback))
            }
        };
        let Some(catalog) = cx.doc.metadata_catalog() else {
            self.save_error = "라이브러리에서만 할 수 있습니다.".into();
            return false;
        };
        let existing = self.session_id.clone();
        let outcome = match (existing, ident) {
            (None, Some(ident)) => create(
                catalog,
                &ident,
                &self.snapshot,
                &self.scope,
                &dust::options_json(&self.options()),
                &self.entries,
                self.current_index(),
            )
            .map(|(id, revision)| {
                self.session_id = Some(id);
                self.revision = revision;
            }),
            (Some(session), _) => {
                let changes: Vec<(usize, Value)> = self.dirty.iter().map(|i| (*i, self.entries[*i].clone())).collect();
                save(catalog, &session, self.revision, &changes, self.current_index(), false)
                    .map(|revision| self.revision = revision)
            }
            (None, None) => Ok(()),
        };
        if let Err(StoreError(error)) = outcome {
            self.save_error = error;
            self.running = false;
            self.cancel.store(true, Ordering::Relaxed);
            self.update_controls();
            return false;
        }
        self.dirty.clear();
        self.save_error.clear();
        self.refresh_saved(cx);
        self.update_controls();
        true
    }

    /// Port of `show_work`: a saved work loaded.
    fn show_work(&mut self, cx: &mut AppContext, ident: String, loaded: Loaded) {
        let (header, entries, damaged) = loaded;
        self.session_id = Some(ident);
        self.revision = header["revision"].as_i64().unwrap_or(0);
        self.snapshot = header["snapshot"].clone();
        self.entries = entries;
        self.dirty = damaged.iter().copied().collect();
        self.save_error.clear();
        let options = serde_json::from_value::<grainy_dust::Options>(header["options"].clone()).unwrap_or_default();
        let scope = header["scope"].as_str().unwrap_or("selection").to_owned();
        self.scope = scope.clone();
        self.sensitivity = options.sensitivity;
        self.minimum = options.minimum;
        self.maximum = options.maximum;
        self.polarity = options.polarity.clone();
        self.detailed = options.detailed;
        self.reduce_patterns = options.reduce_patterns;
        self.soft = options.soft;
        self.suppress_grain = options.suppress_grain;
        self.scratches = options.scratches;
        self.scratch_width = options.scratch_width;
        let mut note = tr_with("저장한 대상 {0}장 · 영상 제외", &[&thousands(self.entries.len())]);
        if scope == "folder" {
            note.push('\n');
            note.push_str(self.snapshot["folder"].as_str().unwrap_or(""));
            note.push_str(&tr(" · 하위 폴더 제외"));
        } else {
            note.push_str(&tr(" · 선택한 사진"));
        }
        self.scope_note = note;
        self.current = -1;
        if !self.entries.is_empty() {
            let index = header["current_index"].as_i64().unwrap_or(0).clamp(0, self.entries.len() as i64 - 1);
            self.set_current(index);
        }
        let done = self.entries.iter().filter(|e| e["state"] != "pending").count();
        self.progress = (0, self.entries.len().max(1), done);
        self.status = if damaged.is_empty() {
            tr("저장한 검토를 불러왔습니다. 완료한 검출을 반복하지 않습니다.")
        } else {
            tr_with("저장한 검토를 불러왔습니다. 확인 필요 {0}장.", &[&thousands(damaged.len())])
        };
        self.refresh_saved(cx);
        self.update_controls();
        if !self.dirty.is_empty() {
            self.persist(cx, &[], false);
        }
    }

    /// Port of `load_stopped`.
    fn load_stopped(&mut self, cx: &AppContext, message: String) {
        let done = self.entries.iter().filter(|e| e["state"] != "pending").count();
        self.progress = (0, self.entries.len().max(1), done);
        self.status = message;
        self.refresh_saved(cx);
        self.update_controls();
    }

    /// Port of `stop` (검출 중단 / 불러오기 중단).
    pub fn stop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.running = false;
        self.status = if self.loading {
            tr("불러오기를 중단하는 중…")
        } else {
            tr("검출을 중단하는 중… 완료한 사진의 결과는 유지됩니다.")
        };
    }

    /// Port of the 예 of `forget_work`'s question: the work's candidates and reviews are deleted.
    pub fn forget_confirmed(&mut self, cx: &mut AppContext) {
        let Some(session) = self.session_id.clone() else { return };
        let Some(catalog) = cx.doc.metadata_catalog() else { return };
        if let Err(StoreError(error)) = delete(catalog, &session, self.revision) {
            self.status = error;
            return;
        }
        self.new_work(cx);
    }

    /// Port of `update_row` for the window's progress after a detection.
    fn progress_after_scan(&mut self) {
        let done = self.entries.iter().filter(|e| e["state"] != "pending").count();
        self.progress = (0, self.entries.len(), done);
    }
}

// ---- the jobs and the actions that need the shared dialog ---------------------------------------------------

/// Port of `DustBatchDialog.__init__`'s end: the dialog, with the saved work of the same target loaded.
pub fn open(cx: &mut AppContext) -> Result<Shared, String> {
    let batch = DustBatch::new(cx)?;
    let key = grainy_dust::store::target_key(&batch.snapshot, &batch.scope).ok();
    let found = cx
        .doc
        .metadata_catalog()
        .map(sessions)
        .and_then(Result::ok)
        .unwrap_or_default()
        .into_iter()
        .find(|s| s["target_key"].as_str() == key.as_deref())
        .and_then(|s| s["id"].as_str().map(str::to_owned));
    let shared = Rc::new(RefCell::new(batch));
    LAST.with(|last| *last.borrow_mut() = Rc::downgrade(&shared));
    if let Some(ident) = found {
        load_work(&shared, cx, &ident);
    }
    Ok(shared)
}

thread_local! {
    static LAST: RefCell<std::rc::Weak<RefCell<DustBatch>>> = const { RefCell::new(std::rc::Weak::new()) };
}

/// The batch window [`open`] opened last on this thread, while it lives (tests drive the shown window).
pub fn last_opened() -> Option<Shared> {
    LAST.with(|last| last.borrow().upgrade())
}

/// Port of `work_changed`: a saved work chosen, or 새 작업.
pub fn work_changed(shared: &Shared, cx: &mut AppContext, index: usize) {
    let ident = shared.borrow().works.get(index).and_then(|w| w.1.clone());
    match ident {
        Some(ident) => load_work(shared, cx, &ident),
        None => shared.borrow_mut().new_work(cx),
    }
}

/// Port of `load_work`: a saved work read on a job (the shown list stays until it is complete).
pub fn load_work(shared: &Shared, cx: &mut AppContext, ident: &str) {
    {
        let b = shared.borrow();
        if b.busy || b.running || !b.save_error.is_empty() {
            return;
        }
    }
    commit_open_photo(cx);
    let mut b = shared.borrow_mut();
    if b.session_id.is_some() && !b.persist(cx, &[], false) {
        return;
    }
    let Some(directory) = cx.doc.metadata_catalog().map(|c| c.directory().to_path_buf()) else { return };
    b.busy = true;
    b.loading = true;
    b.cancel = Arc::new(AtomicBool::new(false));
    let cancel = b.cancel.clone();
    b.load_token += 1;
    let token = b.load_token;
    b.progress = (0, 0, 0);
    b.status = tr("저장한 검토를 불러오는 중…");
    b.update_controls();
    let progress = b.progress_shared.clone();
    drop(b);
    let ident = ident.to_owned();
    let work_ident = ident.clone();
    let work = move |_: &crate::jobs::JobHandle| {
        let mut report = |done: usize, total: usize| {
            *progress.lock().unwrap_or_else(|e| e.into_inner()) = Some((done, total));
        };
        read_work(&directory, &work_ident, &cancel, &mut report)
    };
    let shared = shared.clone();
    cx.jobs.spawn(
        "dust_load",
        tr("저장한 검토를 불러오는 중…"),
        work,
        move |cx: &mut AppContext, result| {
            let mut b = shared.borrow_mut();
            if b.closed || token != b.load_token {
                return;
            }
            b.busy = false;
            b.loading = false;
            match result {
                Ok(Some(loaded)) if !b.cancel.load(Ordering::Relaxed) => b.show_work(cx, ident, loaded),
                Ok(_) => b.load_stopped(cx, tr("불러오기를 중단했습니다. 기존 작업은 유지됩니다.")),
                Err(error) => b.load_stopped(cx, format!("검토를 불러오지 못했습니다: {error}")),
            }
        },
    );
}

/// Port of `start` (남은 N장 검출); `only` detects one entry (이 사진 다시 검출).
pub fn start(shared: &Shared, cx: &mut AppContext, only: Option<usize>) {
    {
        let mut b = shared.borrow_mut();
        if b.closed || b.busy || b.running || !b.save_error.is_empty() {
            return;
        }
        if let Err(message) = dust::validate(&b.options()) {
            b.status = message;
            return;
        }
    }
    commit_open_photo(cx);
    {
        let mut b = shared.borrow_mut();
        if !b.persist(cx, &[], true) {
            return;
        }
        b.scan_indices = match only {
            Some(i) => vec![i],
            None => (0..b.entries.len()).collect(),
        };
        b.cancel = Arc::new(AtomicBool::new(false));
        b.running = true;
        b.update_controls();
    }
    start_next(shared, cx);
}

/// Port of `start_next`: the next pending entry, detected on a job.
pub fn start_next(shared: &Shared, cx: &mut AppContext) {
    let mut b = shared.borrow_mut();
    if b.closed {
        return;
    }
    if b.cancel.load(Ordering::Relaxed) {
        b.running = false;
        b.update_controls();
        return;
    }
    let Some(index) = b.scan_indices.iter().copied().find(|i| b.entries[*i]["state"] == "pending") else {
        b.running = false;
        b.status = tr("검출이 끝났습니다. 사진을 검토한 뒤 적용하세요.");
        b.update_controls();
        return;
    };
    let record = b.entries[index]["record"].clone();
    let options = b.options();
    let cancel = b.cancel.clone();
    let verified = cx
        .doc
        .metadata_catalog()
        .map_or(Err("라이브러리에서만 할 수 있습니다.".to_owned()), |c| verify_source(c, &record));
    if let Err(error) = verified {
        b.entries[index]["state"] = json!("failed");
        b.entries[index]["error"] = json!(error);
        b.persist(cx, &[index], false);
        drop(b);
        start_next(shared, cx);
        return;
    }
    b.active_index = index as i64;
    b.busy = true;
    b.entries[index]["state"] = json!("scanning");
    b.update_controls();
    let name = Path::new(record["path"].as_str().unwrap_or_default())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    b.status = tr_with("{0} / {1} · {2} 검출 중…", &[&thousands(index + 1), &thousands(b.entries.len()), &name]);
    *b.progress_shared.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let progress = b.progress_shared.clone();
    drop(b);
    let work_record = record.clone();
    let work_options = options.clone();
    let work = move |handle: &crate::jobs::JobHandle| -> Result<Option<Value>, String> {
        let run = || -> Result<Value, String> {
            let path = std::path::PathBuf::from(work_record["path"].as_str().unwrap_or_default());
            let settings = grainy_core::Settings(work_record["settings"].as_object().cloned().unwrap_or_default());
            let prepared = dust::prepare_source(&path, &settings, &cancel)?;
            let gray = prepared.gray;
            let check = || cancel.load(Ordering::Relaxed);
            let mut report = |done: usize, total: usize| {
                *progress.lock().unwrap_or_else(|e| e.into_inner()) = Some((done, total));
                handle.progress(done, total, "");
            };
            let detection =
                grainy_dust::detect(&gray, &work_options, Some(&check), Some(&mut report), 640, MAX_CANDIDATES)
                    .map_err(|e| match e {
                        grainy_dust::Error::Cancelled => CANCELLED.to_owned(),
                        other => other.to_string(),
                    })?;
            if cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.into());
            }
            let now = grainy_catalog::preview::signature(&path);
            if !now.is_some_and(|s| grainy_core::settings::equivalent(&s, &work_record["fingerprint"])) {
                return Err("분석 중 원본이 바뀌었습니다. 다시 검출해 주세요.".into());
            }
            Ok(dust::detection_json(&detection))
        };
        match run() {
            Ok(v) => Ok(Some(v)),
            Err(e) if e.contains(CANCELLED) => Ok(None),
            Err(e) => Err(e),
        }
    };
    let shared_done = shared.clone();
    cx.jobs.spawn(
        "dust",
        tr("먼지 검출"),
        work,
        move |cx: &mut AppContext, result: Result<Option<Value>, String>| {
            let running = {
                let mut b = shared_done.borrow_mut();
                if b.closed {
                    return;
                }
                b.busy = false;
                match result {
                    Ok(result) => {
                        match result.filter(|_| !b.cancel.load(Ordering::Relaxed)) {
                            None => {
                                b.entries[index]["state"] = json!("pending");
                                b.running = false;
                                b.status = tr("검출을 중단했습니다. 완료한 사진은 검토할 수 있습니다.");
                            }
                            Some(result) => {
                                let verified =
                                    cx.doc.metadata_catalog().map_or(
                                        Err("라이브러리에서만 할 수 있습니다.".to_owned()),
                                        |c| verify_source(c, &record),
                                    );
                                match verified {
                                    Err(error) => {
                                        b.entries[index]["state"] = json!("failed");
                                        b.entries[index]["error"] = json!(error);
                                    }
                                    Ok(()) => {
                                        let options = dust::options_json(&options);
                                        let entry = &mut b.entries[index];
                                        entry["state"] = json!("ready");
                                        entry["result"] = result;
                                        entry["options"] = options.clone();
                                        let _ = cx.doc.save_preference("dust_detection_options", &options);
                                    }
                                }
                            }
                        }
                        b.persist(cx, &[index], false);
                        b.update_controls();
                        b.progress_after_scan();
                    }
                    Err(error) => {
                        b.entries[index]["state"] = json!("failed");
                        b.entries[index]["error"] = json!(error);
                        b.persist(cx, &[index], false);
                        b.update_controls();
                    }
                }
                b.running
            };
            if running {
                start_next(&shared_done, cx);
            }
        },
    );
}

/// The job's progress for the dialog's bar (`scan_progress`, `load_progress`).
pub fn poll_progress(b: &mut DustBatch) {
    if b.closed || !b.busy {
        return;
    }
    if let Some((done, total)) = *b.progress_shared.lock().unwrap_or_else(|e| e.into_inner()) {
        b.progress = (0, total.max(1), done);
    }
}

/// Port of `retry` (이 사진 다시 검출): the current entry captured again and detected.
pub fn retry(shared: &Shared, cx: &mut AppContext) {
    {
        let b = shared.borrow();
        if b.busy || b.running {
            return;
        }
    }
    commit_open_photo(cx);
    let index = {
        let mut b = shared.borrow_mut();
        let Ok(index) = usize::try_from(b.current) else { return };
        if index >= b.entries.len() {
            return;
        }
        let id = b.entries[index]["record"]["id"].as_i64().unwrap_or(0);
        let captured = cx
            .doc
            .metadata_catalog()
            .map_or(Err("라이브러리에서만 할 수 있습니다.".to_owned()), |c| capture_record(c, id));
        match captured {
            Err(error) => {
                b.status = error;
                return;
            }
            Ok(record) => b.entries[index] = pending_entry(record),
        }
        b.dirty.insert(index);
        index
    };
    start(shared, cx, Some(index));
}

/// Port of `review_selected` (선택한 사진 검토): the photo's review dialog over this one (returned, `None` when
/// none opened).
pub fn review_selected(shared: &Shared, cx: &mut AppContext) -> Option<dust::Shared> {
    let index = {
        let b = shared.borrow();
        if b.busy || b.running || b.closed {
            return None;
        }
        let index = usize::try_from(b.current).ok()?;
        let entry = b.entries.get(index)?;
        if entry["state"] != "ready" && entry["state"] != "reviewed" {
            return None;
        }
        index
    };
    commit_open_photo(cx);
    let entry = shared.borrow().entries[index].clone();
    let verified =
        cx.doc.metadata_catalog().map_or(Err("라이브러리에서만 할 수 있습니다.".to_owned()), |c| {
            verify_source(c, &entry["record"])
        });
    if let Err(error) = verified {
        let mut b = shared.borrow_mut();
        b.entries[index]["state"] = json!("failed");
        b.entries[index]["error"] = json!(error);
        b.entries[index]["operation"] = Value::Null;
        b.persist(cx, &[index], false);
        b.update_controls();
        return None;
    }
    let review = BatchReview {
        record: entry["record"].clone(),
        cached: Some(json!({"result": entry["result"], "options": entry["options"]})),
        review_state: (!entry["review"].is_null()).then(|| entry["review"].clone()),
    };
    let dialog = Rc::new(RefCell::new(DustReview::new(cx, Some(review))));
    let batch = shared.clone();
    let opened = dialog.clone();
    cx.open_dialog(Box::new(dust::DustDialog::new(dialog, move |cx, review| reviewed(&batch, cx, index, review))));
    Some(opened)
}

/// The review dialog of entry `index` accepted (the rest of `review_selected`).
pub fn reviewed(shared: &Shared, cx: &mut AppContext, index: usize, review: &DustReview) {
    let mut b = shared.borrow_mut();
    if review.valid_source(cx) {
        let entry = &mut b.entries[index];
        entry["state"] = json!("reviewed");
        entry["operation"] = review.operation.clone().unwrap_or(Value::Null);
        entry["review"] = review.review_state.clone().unwrap_or(Value::Null);
        entry["result"] = review.result.clone().unwrap_or(Value::Null);
        entry["options"] = review.detect_options.clone();
        b.persist(cx, &[index], false);
        b.status = tr("검토 내용을 보관했습니다. 아래 적용 버튼을 눌러야 사진에 반영됩니다.");
    }
    b.update_controls();
}

/// Port of `review_next` (다음 미검토 사진).
pub fn review_next(shared: &Shared, cx: &mut AppContext) -> Option<dust::Shared> {
    let found = {
        let b = shared.borrow();
        if b.busy || b.running {
            return None;
        }
        let n = b.entries.len() as i64;
        let current = b.current;
        let order: Vec<i64> = (current + 1..n).chain(0..current + 1).collect();
        order.into_iter().filter(|i| *i >= 0).find(|i| b.entries[*i as usize]["state"] == "ready")
    };
    let index = found?;
    shared.borrow_mut().set_current(index);
    review_selected(shared, cx)
}

/// Port of `apply` (검토한 N장 · 먼지 N개 적용): one command batch; the entries become 적용 완료 in the same
/// transaction.
pub fn apply(shared: &Shared, cx: &mut AppContext) {
    {
        let b = shared.borrow();
        if b.busy || b.running || b.closed || !b.save_error.is_empty() {
            return;
        }
    }
    commit_open_photo(cx);
    let mut b = shared.borrow_mut();
    if !b.persist(cx, &[], true) {
        return;
    }
    let session = b.session_id.clone().unwrap_or_default();
    let (start_revision, current) = (b.revision, b.current);
    let entries = b.entries.clone();
    let mut updated: Vec<(usize, Value)> = Vec::new();
    let mut revision = start_revision;
    let result = {
        let Some(catalog) = cx.doc.metadata_catalog() else { return };
        let mut hook = |ids: &[i64]| -> grainy_service::Result<()> {
            updated.clear();
            for (i, entry) in entries.iter().enumerate() {
                if entry["record"]["id"].as_i64().is_some_and(|id| ids.contains(&id)) {
                    let mut entry = entry.clone();
                    entry["state"] = json!("applied");
                    updated.push((i, entry));
                }
            }
            revision = save(catalog, &session, start_revision, &updated, current, true)
                .map_err(|StoreError(e)| grainy_service::Error::Value(e))?;
            Ok(())
        };
        apply_reviewed(catalog, &entries, &mut hook)
    };
    let ids = match result {
        Ok(ids) => ids,
        Err(error) => {
            b.status = error;
            return;
        }
    };
    b.revision = revision;
    for (i, entry) in updated {
        b.entries[i] = entry;
    }
    b.refresh_saved(cx);
    let ids: Vec<u64> = ids.iter().map(|i| *i as u64).collect();
    let _ = cx.doc.refresh_command_edits(&ids);
    cx.set_status(tr_with("먼지 일괄 제거 · {0}장 적용", &[&thousands(ids.len())]));
    b.status =
        tr_with("{0}장에 적용했습니다. 최근 먼지 일괄 적용 취소로 함께 되돌릴 수 있습니다.", &[&thousands(ids.len())]);
    b.update_controls();
}

/// Port of `restore` (최근 먼지 일괄 적용 취소 / 다시 적용): the photos of the batch become pending again.
pub fn restore_batch(shared: &Shared, cx: &mut AppContext, redo: bool) {
    {
        let b = shared.borrow();
        if b.busy || b.running || !b.save_error.is_empty() {
            return;
        }
    }
    commit_open_photo(cx);
    let mut b = shared.borrow_mut();
    let session = b.session_id.clone();
    let (start_revision, current) = (b.revision, b.current);
    let entries = b.entries.clone();
    let mut updated: Vec<(usize, Value)> = Vec::new();
    let mut revision = start_revision;
    let result = {
        let Some(catalog) = cx.doc.metadata_catalog() else { return };
        let mut hook = |ids: &[i64]| -> grainy_service::Result<()> {
            updated.clear();
            for (i, entry) in entries.iter().enumerate() {
                let Some(id) = entry["record"]["id"].as_i64().filter(|id| ids.contains(id)) else { continue };
                let record = capture_record(catalog, id).map_err(grainy_service::Error::Value)?;
                updated.push((i, pending_entry(record)));
            }
            if let Some(session) = &session {
                revision = save(catalog, session, start_revision, &updated, current, true)
                    .map_err(|StoreError(e)| grainy_service::Error::Value(e))?;
            }
            Ok(())
        };
        restore(catalog, redo, &mut hook)
    };
    let ids = match result {
        Ok(ids) => ids,
        Err(error) => {
            b.status = error;
            return;
        }
    };
    b.revision = revision;
    let ids: Vec<u64> = ids.iter().map(|i| *i as u64).collect();
    let _ = cx.doc.refresh_command_edits(&ids);
    cx.set_status(tr("먼지 일괄 적용을 복원했습니다."));
    for (i, entry) in updated {
        b.entries[i] = entry;
    }
    b.refresh_saved(cx);
    b.status = if redo {
        tr("먼지 일괄 적용을 다시 실행했습니다.")
    } else {
        tr("먼지 일괄 적용을 취소했습니다.")
    };
    b.update_controls();
}

/// Port of `forget_work` (작업 기록 삭제): asks first.
pub fn forget_work(shared: &Shared, _cx: &mut AppContext) {
    let mut b = shared.borrow_mut();
    if b.session_id.is_none() || b.busy || b.running || !b.save_error.is_empty() {
        return;
    }
    b.question = Some(Question::Forget);
}

/// Port of `done` (닫기): the work is saved; when that fails, asks whether to close anyway. Returns whether the
/// dialog closes now.
pub fn close(shared: &Shared, cx: &mut AppContext) -> bool {
    let mut b = shared.borrow_mut();
    if b.closed {
        return true;
    }
    if b.session_id.is_some() || !b.save_error.is_empty() {
        let create = !b.save_error.is_empty();
        if !b.persist(cx, &[], create) {
            b.question = Some(Question::DiscardOnClose);
            return false;
        }
    }
    b.closed = true;
    b.cancel.store(true, Ordering::Relaxed);
    b.running = false;
    true
}

// ---- the window ---------------------------------------------------------------------------------------------

/// The batch window.
pub struct DustBatchDialog {
    pub shared: Shared,
}

impl Dialog for DustBatchDialog {
    fn id(&self) -> &'static str {
        "dust_batch"
    }

    fn show(&mut self, ctx: &egui::Context, cx: &mut AppContext) -> bool {
        poll_progress(&mut self.shared.borrow_mut());
        let question = self.shared.borrow_mut().question.take();
        match question {
            Some(Question::Forget) => {
                let shared = self.shared.clone();
                cx.open_dialog(Box::new(Confirm::new(
                    "dust_forget",
                    tr("검토 기록 삭제"),
                    tr("이 작업의 검출 후보와 검토 기록을 삭제할까요? 사진에 적용한 보정과 실행 취소 정보는 유지됩니다."),
                    move |cx| shared.borrow_mut().forget_confirmed(cx),
                )));
            }
            Some(Question::DiscardOnClose) => {
                let shared = self.shared.clone();
                cx.open_dialog(Box::new(Confirm::new(
                    "dust_discard",
                    tr("검토 저장 실패"),
                    tr("최근 검토 내용을 저장하지 못했습니다. 저장되지 않은 변경을 버리고 닫을까요? 이전에 저장된 작업은 유지됩니다."),
                    move |_| {
                        let mut b = shared.borrow_mut();
                        b.closed = true;
                        b.cancel.store(true, Ordering::Relaxed);
                        b.running = false;
                    },
                )));
            }
            None => {}
        }
        if self.shared.borrow().closed {
            return false;
        }
        let mut close_asked = false;
        let screen = ctx.content_rect();
        let size = Vec2::new(940.0f32.min(screen.width() - 20.0), 720.0f32.min(screen.height() - 20.0));
        let modal = egui::Modal::new(egui::Id::unique("dust_batch_dialog")).frame(super::frame()).show(ctx, |ui| {
            ui.set_width(size.x - 32.0);
            ui.set_height(size.y - 32.0);
            close_asked = body(&self.shared, ui, cx);
        });
        if modal.should_close() && !cx.dialog_open("dust") && !cx.dialog_open("dust_forget") {
            close_asked = true;
        }
        if self.shared.borrow().busy || self.shared.borrow().running {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        if close_asked {
            return !close(&self.shared, cx);
        }
        true
    }
}

fn plain(ui: &mut egui::Ui, text: &str) {
    widgets::label(ui, text, t::TEXT, t::FONT_SIZE);
}

fn spin(ui: &mut egui::Ui, value: &mut i32, range: std::ops::RangeInclusive<i32>, suffix: &str, enabled: bool) -> bool {
    ui.add_enabled(enabled, egui::DragValue::new(value).range(range).suffix(suffix).speed(0.2).max_decimals(0))
        .changed()
}

fn check(ui: &mut egui::Ui, value: &mut bool, text: &str, enabled: bool, tip: &str) -> bool {
    let mut changed = false;
    ui.add_enabled_ui(enabled, |ui| {
        let response = widgets::checkbox(ui, value, text);
        let response = if tip.is_empty() { response } else { response.on_hover_text(tip) };
        changed = response.changed();
    });
    changed
}

/// The window's contents; returns whether 닫기 was pressed.
fn body(shared: &Shared, ui: &mut egui::Ui, cx: &mut AppContext) -> bool {
    let mut close = false;
    ui.spacing_mut().item_spacing.y = 5.0;
    ui.label(egui::RichText::new(tr("여러 사진의 먼지 감지")).font(t::bold(14.0)).color(t::BRAND));
    ui.label(
        egui::RichText::new(tr("사진별로 찾고, 확인한 먼지만 제거"))
            .font(t::font(20.0))
            .color(Color32::from_rgb(0xea, 0xd4, 0xb1)),
    );
    plain(ui, &tr("검출 → 사진별 검토 → 검토한 사진에 적용\n검출 결과와 완료한 검토는 자동 저장됩니다. 닫거나 앱을 다시 실행해도 이어갈 수 있습니다."));
    // 저장한 작업.
    let (works, work, selector_enabled, new_enabled, forget_enabled) = {
        let b = shared.borrow();
        (
            b.works.clone(),
            b.work,
            b.enabled(cx, "work_selector"),
            b.enabled(cx, "new_button"),
            b.enabled(cx, "forget_button"),
        )
    };
    let mut chosen = None;
    ui.horizontal(|ui| {
        plain(ui, &tr("저장한 작업"));
        ui.add_enabled_ui(selector_enabled, |ui| {
            let shown = works.get(work).map(|w| w.0.clone()).unwrap_or_default();
            egui::ComboBox::from_id_salt("dust_works").width(420.0).selected_text(shown).show_ui(ui, |ui| {
                for (i, (text, _, tip)) in works.iter().enumerate() {
                    let response = ui.selectable_label(i == work, text);
                    let response = if tip.is_empty() { response } else { response.on_hover_text(tip) };
                    if response.clicked() && i != work {
                        chosen = Some(i);
                    }
                }
            });
        });
        if Button::new(&tr("현재 대상으로 새 작업")).enabled(new_enabled).show(ui).clicked() {
            shared.borrow_mut().new_work(cx);
        }
        if Button::new(&tr("작업 기록 삭제")).enabled(forget_enabled).show(ui).clicked() {
            forget_work(shared, cx);
        }
    });
    if let Some(index) = chosen {
        shared.borrow_mut().work = index;
        work_changed(shared, cx, index);
    }
    // 대상.
    let (scope, scope_enabled) = {
        let b = shared.borrow();
        (b.scope.clone(), b.enabled(cx, "scope"))
    };
    let scopes = [("선택한 사진", "selection"), ("현재 폴더 전체", "folder")];
    let mut new_scope = scope.clone();
    ui.horizontal(|ui| {
        plain(ui, &tr("대상"));
        ui.add_enabled_ui(scope_enabled, |ui| {
            let shown = scopes.iter().find(|(_, k)| *k == scope).map_or(String::new(), |(l, _)| tr(l));
            egui::ComboBox::from_id_salt("dust_scope").width(300.0).selected_text(shown).show_ui(ui, |ui| {
                for (label, key) in scopes {
                    ui.selectable_value(&mut new_scope, key.to_owned(), tr(label));
                }
            });
        });
    });
    if new_scope != scope {
        shared.borrow_mut().scope_changed(cx, &new_scope);
    }
    let note = shared.borrow().scope_note.clone();
    plain(ui, &note);
    // The detection options.
    {
        let mut b = shared.borrow_mut();
        let b = &mut *b;
        let mut changed = false;
        let e = |n: &str| b.enabled(cx, n);
        let (en_sens, en_min, en_max, en_pol) = (e("sensitivity"), e("minimum"), e("maximum"), e("polarity"));
        let (en_det, en_soft, en_red, en_sup, en_scr, en_width) =
            (e("detailed"), e("soft"), e("reduce_patterns"), e("suppress_grain"), e("scratches"), e("scratch_width"));
        ui.horizontal(|ui| {
            plain(ui, &tr("민감도"));
            changed |= spin(ui, &mut b.sensitivity, 1..=100, "", en_sens);
            plain(ui, &tr("최소 점 크기"));
            changed |= spin(ui, &mut b.minimum, 2..=160, " px", en_min);
            plain(ui, &tr("최대 점 크기"));
            changed |= spin(ui, &mut b.maximum, 2..=160, " px", en_max);
            plain(ui, &tr("종류"));
            let before = b.polarity.clone();
            ui.add_enabled_ui(en_pol, |ui| {
                let shown =
                    dust::POLARITIES.iter().find(|(_, k)| *k == b.polarity).map_or(String::new(), |(l, _)| tr(l));
                egui::ComboBox::from_id_salt("dust_batch_polarity").selected_text(shown).show_ui(ui, |ui| {
                    for (label, key) in dust::POLARITIES {
                        ui.selectable_value(&mut b.polarity, key.to_owned(), tr(label));
                    }
                });
            });
            changed |= b.polarity != before;
        });
        egui::Grid::new("dust_batch_options").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
            changed |= check(ui, &mut b.detailed, &tr("경계·미세 먼지까지 정밀 검색"), en_det, "");
            changed |= check(ui, &mut b.soft, &tr("입자 속 옅은 먼지 추가 검색"), en_soft, &tr("옅은 점을 추가로 찾습니다. 시간이 더 걸리고 무늬도 후보에 포함될 수 있으므로 직접 확인해 주세요."));
            ui.end_row();
            changed |= check(ui, &mut b.reduce_patterns, &tr("무늬 후보 줄이기"), en_red, &tr("반복 무늬와 넓은 물체 주변의 후보를 줄입니다. 실제 먼지나 배경과 붙은 흠집도 놓칠 수 있습니다."));
            changed |= check(ui, &mut b.suppress_grain, &tr("입자 후보 억제"), en_sup, &tr("옅은 점의 검출 기준을 높여 검토 후보를 줄입니다. 옅은 먼지도 놓칠 수 있으므로 후보가 너무 많을 때 켜세요."));
            ui.end_row();
            changed |= check(ui, &mut b.scratches, &tr("긴 흠집·머리카락 후보 찾기"), en_scr, &tr("전선·가지·글자도 잡힐 수 있어 추가 후보를 직접 확인해 주세요. 제거 범위는 가는 모양을 따라갑니다."));
            ui.horizontal(|ui| {
                plain(ui, &tr("흠집 최대 굵기"));
                changed |= spin(ui, &mut b.scratch_width, 2..=40, " px", en_width);
            });
            ui.end_row();
        });
        plain(ui, &tr("조건·대상을 바꾸면 새 작업을 준비합니다. 이전 검토는 위 목록에서 다시 열 수 있습니다."));
        if changed {
            b.reset_entries(cx);
        }
    }
    // The list.
    let list_height = (ui.available_height() - 150.0).max(120.0);
    let headers = [tr("사진"), tr("상태"), tr("검출 후보"), tr("선택한 먼지")];
    let mut clicked = None;
    let mut double = None;
    egui::Frame::new().fill(t::LIST_BG).stroke(egui::Stroke::new(1.0, t::INPUT_BORDER)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let width = ui.available_width();
        let columns = [width - 3.0 * 130.0, 130.0, 130.0, 130.0];
        ui.horizontal(|ui| {
            for (header, w) in headers.iter().zip(columns) {
                ui.add_sized([w - 8.0, 20.0], egui::Label::new(egui::RichText::new(header).color(t::MUTED)));
            }
        });
        let b = shared.borrow();
        egui::ScrollArea::vertical()
            .id_salt("dust_batch_list")
            .max_height(list_height)
            .min_scrolled_height(list_height)
            .show_rows(ui, 22.0, b.entries.len(), |ui, range| {
                for index in range {
                    let row = b.row(index);
                    let selected = b.current == index as i64;
                    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 22.0), egui::Sense::click());
                    let fill = if selected {
                        t::CHECKED
                    } else if index % 2 == 1 {
                        Color32::from_rgb(0x1e, 0x23, 0x29)
                    } else {
                        Color32::TRANSPARENT
                    };
                    ui.painter().rect_filled(rect, 0.0, fill);
                    let mut x = rect.left() + 4.0;
                    for (column, w) in columns.iter().enumerate() {
                        let colour = if column == 1 { b.state_colour(index) } else { t::TEXT };
                        let full = widgets::galley(ui, row[column].clone(), t::font(t::FONT_SIZE), colour);
                        let g = if full.size().x <= w - 8.0 {
                            full
                        } else {
                            widgets::elided(ui, &row[column], t::font(t::FONT_SIZE), colour, w - 8.0)
                        };
                        ui.painter().galley(egui::pos2(x, rect.center().y - g.size().y / 2.0), g, colour);
                        x += w;
                    }
                    let response =
                        response.on_hover_text(if row[5].is_empty() { row[4].clone() } else { row[5].clone() });
                    if response.clicked() {
                        clicked = Some(index);
                    }
                    if response.double_clicked() {
                        double = Some(index);
                    }
                }
            });
    });
    if let Some(index) = clicked {
        shared.borrow_mut().set_current(index as i64);
    }
    if double.is_some() {
        review_selected(shared, cx);
    }
    // The buttons.
    let (scan_text, stop_text, apply_text) = {
        let b = shared.borrow();
        (b.scan_text(), b.stop_text(), b.apply_text())
    };
    let enabled = |name: &str| shared.borrow().enabled(cx, name);
    let (en_scan, en_stop, en_retry, en_review, en_next) = (
        enabled("scan_button"),
        enabled("stop_button"),
        enabled("retry_button"),
        enabled("review_button"),
        enabled("next_button"),
    );
    let (en_undo, en_redo, en_apply) = (enabled("undo_button"), enabled("redo_button"), enabled("apply_button"));
    ui.horizontal(|ui| {
        if Button::new(&scan_text).enabled(en_scan).show(ui).clicked() {
            start(shared, cx, None);
        }
        if Button::new(&stop_text).enabled(en_stop).show(ui).clicked() {
            shared.borrow_mut().stop();
        }
        if Button::new(&tr("이 사진 다시 검출")).enabled(en_retry).show(ui).clicked() {
            retry(shared, cx);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if Button::new(&tr("다음 미검토 사진")).enabled(en_next).show(ui).clicked() {
                review_next(shared, cx);
            }
            if Button::new(&tr("선택한 사진 검토")).enabled(en_review).show(ui).clicked() {
                review_selected(shared, cx);
            }
        });
    });
    let (progress, summary, status, save_note, save_error, save_enabled) = {
        let b = shared.borrow();
        (
            b.progress,
            b.summary(),
            b.status.clone(),
            b.save_note(),
            !b.save_error.is_empty(),
            b.enabled(cx, "save_button"),
        )
    };
    let (_, max, value) = progress;
    let bar = if max == 0 {
        egui::ProgressBar::new(0.0).animate(true)
    } else {
        egui::ProgressBar::new(value as f32 / max as f32)
    };
    ui.add(bar.desired_height(6.0));
    plain(ui, &summary);
    plain(ui, &status);
    ui.horizontal(|ui| {
        plain(ui, &save_note);
        if save_error && Button::new(&tr("검토 저장 다시 시도")).enabled(save_enabled).show(ui).clicked() {
            shared.borrow_mut().persist(cx, &[], true);
        }
    });
    ui.horizontal(|ui| {
        if Button::new(&tr("최근 먼지 일괄 적용 취소")).enabled(en_undo).show(ui).clicked() {
            restore_batch(shared, cx, false);
        }
        if Button::new(&tr("다시 적용")).enabled(en_redo).show(ui).clicked() {
            restore_batch(shared, cx, true);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if widgets::button(ui, &tr("닫기")).clicked() {
                close = true;
            }
            ui.add_space(6.0);
            if Button::new(&apply_text).kind(Kind::Primary).enabled(en_apply).show(ui).clicked() {
                apply(shared, cx);
            }
        });
    });
    close
}
