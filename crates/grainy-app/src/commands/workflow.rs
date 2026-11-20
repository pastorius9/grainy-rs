//! The batch workflow of the 사진 작업 menu (APP_PLAN package 18), ported from `luma/workflow_ui.py`
//! (`WorkflowTools`): automatic tone, colour and white balance for the selected photos (`command_auto_values` /
//! `photo_service.auto_values`, all or nothing, with a progress box and 취소), negative inversion for each photo
//! and for a roll (`invert_negatives`, `invert_roll`), the scan-border crop (`border_crop`), the update to the
//! current processes (`update_process`), exposure matching (`match_exposure`), the metadata presets
//! (`save_metadata`, `apply_metadata`, `delete_metadata`) and the shortcut keyword (`set_keyword`,
//! `apply_keyword`).
//!
//! The values of a batch are computed on a job for every photo first; only when all of them are there are they
//! written, in one transaction (`command_batch.apply`, no batch record, panels the values change switched on): a
//! cancelled or failed batch leaves the catalogue as it was. A written batch is shown through
//! `Document::refresh_command_edits` (thumbnails, and the open photo's state and undo stacks are read again).
//!
//! Errors the oracle's `manager.run` shows in a message box are a [`Notice`]; `show_error` is the error box.

use std::path::{Path, PathBuf};

use grainy_core::Settings;
use grainy_engine::files::{load_photo, Photo};
use grainy_engine::{CameraSource, Externals};
use grainy_service::auto::{auto_records, auto_values, AutoKind, AutoSource};
use grainy_service::Failure;
use ndarray::Array2;
use serde_json::{json, Map, Value};

use super::metadata::{self, parse_stamp, python_repr, selected_ids};
use super::{has_photos, photo_open, writable_library, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::import_errors::Notice;
use crate::dialogs::metadata_preset::{KeywordInput, PresetChoose, PresetSave, Row};
use crate::dialogs::preset_review::show_error;
use crate::dialogs::progress::{ProgressBox, Shared};
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "workflow.auto_tone", label: "자동 톤 (선택한 사진)", enabled: batch_ready, run: auto_tone },
    Command {
        id: "workflow.auto_color", label: "자동 색 (선택한 사진)", enabled: batch_ready, run: auto_color
    },
    Command { id: "workflow.auto_white", label: "자동 화이트 밸런스", enabled: batch_ready, run: auto_white },
    Command {
        id: "workflow.invert_negatives",
        label: "네거티브 반전 (선택한 사진)",
        enabled: batch_ready,
        run: invert_negatives,
    },
    Command {
        id: "workflow.invert_roll",
        label: "네거티브 반전 · 한 롤로 맞춤 (선택한 사진)",
        enabled: batch_ready,
        run: invert_roll,
    },
    Command {
        id: "workflow.border_crop",
        label: "스캔 테두리 자동 자르기 (선택한 사진)",
        enabled: batch_ready,
        run: border_crop,
    },
    Command {
        id: "workflow.update_process",
        label: "최신 처리 방식으로 업데이트 (선택한 사진)",
        enabled: has_photos,
        run: update_process,
    },
    Command {
        id: "workflow.match_exposure",
        label: "현재 사진에 총노출 맞추기",
        enabled: photo_open,
        run: match_exposure,
    },
    Command {
        id: "workflow.metadata_save",
        label: "메타데이터 프리셋 저장…",
        enabled: preset_photo,
        run: metadata_save,
    },
    Command {
        id: "workflow.metadata_apply",
        label: "메타데이터 프리셋 적용…",
        enabled: preset_photos,
        run: metadata_apply,
    },
    Command {
        id: "workflow.metadata_delete",
        label: "메타데이터 프리셋 삭제…",
        enabled: preset_photos,
        run: metadata_delete,
    },
    Command {
        id: "workflow.keyword_set", label: "바로가기 키워드 설정…", enabled: preset_photos, run: keyword_set
    },
    Command {
        id: "workflow.keyword_apply", label: "바로가기 키워드 적용", enabled: preset_photos, run: keyword_apply
    },
];

/// The job kind of the batches: one runs at a time (`WorkflowTools.cancel`).
const KIND: &str = "workflow";

/// Tests set it to start the next automatic batch as if 취소 had been pressed before the first photo (a job of
/// three small photos is over before a test could press it).
pub static CANCEL_NEXT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What the feature keeps: the errors it showed (the oracle's message boxes), for tests and the status line.
#[derive(Default)]
pub struct WorkflowState {
    /// The texts of the error boxes, oldest first.
    pub errors: Vec<String>,
}

fn batch_ready(cx: &AppContext) -> bool {
    has_photos(cx) && !cx.jobs.running(KIND)
}

fn preset_photo(cx: &AppContext) -> bool {
    writable_library(cx) && photo_open(cx)
}

fn preset_photos(cx: &AppContext) -> bool {
    writable_library(cx) && has_photos(cx)
}

// ---- shared helpers -------------------------------------------------------------------------------------------------

/// `manager.run`: an error of an action is a message box.
fn failed(cx: &mut AppContext, message: &str) {
    cx.state::<WorkflowState>().errors.push(message.to_owned());
    cx.open_dialog(Box::new(Notice {
        id: "workflow_error",
        title: tr("작업을 완료하지 못했습니다"),
        text: tr(message),
    }));
}

/// `show_error(message)`: the status line and the error box with the last line of the message.
fn error_box(cx: &mut AppContext, message: &str) {
    let last = message.trim().lines().last().unwrap_or("").to_owned();
    cx.state::<WorkflowState>().errors.push(last);
    show_error(cx, message);
}

/// Python's `f'{n:,}'`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `MainWindow.commit()`: the pending change becomes a step and is stored.
fn commit_pending(cx: &mut AppContext) {
    cx.doc.edit_mut().commit("보정 변경");
    cx.flush();
}

/// A photo's catalogue row (`catalog.photo(id)`); a session without a catalogue gives the same fields from what
/// it knows.
fn row(cx: &AppContext, id: i64) -> Option<Value> {
    if let Some(catalog) = cx.doc.metadata_catalog() {
        return catalog.photo(id).ok().flatten();
    }
    let id = u64::try_from(id).ok()?;
    let photo = cx.doc.photos().iter().find(|p| p.id == id)?;
    let settings = cx.doc.stored_settings_of(id).ok()?;
    Some(json!({
        "id": id,
        "path": photo.path.to_string_lossy(),
        "settings": Value::Object(settings.0),
        "info": cx.doc.photo_info(id),
        "keywords": "",
        "user_metadata": {},
    }))
}

/// `WorkflowTools.records`: the pending change is stored, then the rows of the selected photos.
fn records(cx: &mut AppContext) -> Vec<Value> {
    commit_pending(cx);
    selected_ids(cx).into_iter().filter_map(|id| row(cx, id)).collect()
}

fn settings_of(record: &Value) -> Settings {
    Settings(record["settings"].as_object().cloned().unwrap_or_default())
}

fn id_of(record: &Value) -> i64 {
    record["id"].as_i64().unwrap_or(0)
}

/// `WorkflowTools.apply_edits(records, values, label)` without a retarget: every changed photo written in one
/// transaction (`command_batch.apply(..., remember=False)`), the window told to read them again, the status
/// line `{label} · {n}장 변경`. Returns the changed ids.
pub fn apply_edits(
    cx: &mut AppContext,
    records: &[Value],
    values: Vec<Settings>,
    label: &str,
) -> Result<Vec<i64>, String> {
    if records.is_empty() {
        return Ok(Vec::new());
    }
    let ids = match cx.doc.metadata_catalog().filter(|_| !cx.doc.read_only()) {
        Some(catalog) => {
            let values: Vec<Value> = values.into_iter().map(|s| Value::Object(s.0)).collect();
            let options = grainy_service::batch::ApplyOptions {
                label: label.to_owned(),
                remember: false,
                switch: true,
                ..Default::default()
            };
            let ids =
                grainy_service::batch::apply(catalog, records, &values, &options, None).map_err(|e| e.to_string())?;
            let wide: Vec<u64> = ids.iter().filter_map(|i| u64::try_from(*i).ok()).collect();
            cx.doc.refresh_command_edits(&wide)?;
            ids
        }
        None => {
            // A session or a read-only library keeps the edits in the window, one step per photo.
            let ids: Vec<i64> = records.iter().map(id_of).collect();
            let wide: Vec<u64> = ids.iter().filter_map(|i| u64::try_from(*i).ok()).collect();
            cx.doc.edit_photos(&wide, &values, label)?;
            ids
        }
    };
    cx.before = false;
    cx.status = tr_with("{0} · {1}장 변경", &[&tr(label), &thousands(ids.len())]);
    Ok(ids)
}

/// `apply_edits` as `manager.run` calls it: a refusal is a message box.
fn apply_or_fail(cx: &mut AppContext, records: &[Value], values: Vec<Settings>, label: &str) {
    if let Err(message) = apply_edits(cx, records, values, label) {
        failed(cx, &message);
    }
}

// ---- loading, as `photo_service.auto_values` does -------------------------------------------------------------------

fn failure(kind: &str, message: impl ToString) -> Failure {
    Failure::new(kind, message.to_string())
}

fn files_failure(error: &grainy_engine::files::Error) -> Failure {
    use grainy_engine::files::Error as E;
    match error {
        E::Io(e) | E::Codecs(grainy_codecs::Error::Io(e)) => Failure::io(e),
        _ => failure("ValueError", error),
    }
}

fn auto_failure(error: &grainy_engine::auto::Error) -> Failure {
    failure("ValueError", error)
}

/// `preview_store.load_offline(directory, id, settings)`: the photo's offline preview as a develop source.
fn offline(directory: &Path, ident: i64, settings: &Settings) -> Result<Photo, Failure> {
    use ndarray::Array3;
    let convert = |pixels: grainy_catalog::preview::Pixels, from: &str, to: &str| {
        let image = Array3::from_shape_vec(pixels.shape, pixels.values.clone())
            .map_err(|e| grainy_catalog::Error::Invalid(e.to_string()))?;
        let source = grainy_cms::profile(from).map_err(|e| grainy_catalog::Error::Invalid(e.to_string()))?;
        let target = grainy_cms::profile(to).map_err(|e| grainy_catalog::Error::Invalid(e.to_string()))?;
        let converted =
            grainy_cms::convert(&image, &source, &target).map_err(|e| grainy_catalog::Error::Invalid(e.to_string()))?;
        Ok(grainy_catalog::preview::Pixels {
            shape: pixels.shape,
            values: converted.into_raw_vec_and_offset().0,
            half: false,
        })
    };
    let (pixels, info) =
        grainy_catalog::preview::load_offline(directory, ident, settings, convert).map_err(|e| match e {
            grainy_catalog::Error::Io(e) => Failure::io(&e),
            other => failure("ValueError", other),
        })?;
    let image = Array3::from_shape_vec(pixels.shape, pixels.values).map_err(|e| failure("ValueError", e))?;
    let info = info.as_object().cloned().unwrap_or_default();
    if info.get("raw_native").is_some_and(grainy_core::settings::truthy) {
        let raw: grainy_raw::rawcolor::CameraInfo =
            serde_json::from_value(info.get("raw_info").cloned().unwrap_or_default())
                .map_err(|e| failure("ValueError", e))?;
        return Ok(Photo::Camera(Box::new(CameraSource::new(image, raw).map_err(|e| failure("ValueError", e))?)));
    }
    Ok(Photo::Linear(image))
}

/// `resize_float(source, longest)` of a develop source (a camera source keeps its calibration).
fn reduced(photo: Photo, longest: usize) -> Result<Photo, Failure> {
    let fail = |e: grainy_codecs::Error| failure("ValueError", e);
    Ok(match photo {
        Photo::Linear(image) => Photo::Linear(grainy_codecs::resize_float(&image, longest).map_err(fail)?),
        Photo::Camera(camera) => {
            let (h, w, _) = camera.pixels().dim();
            if h.max(w) <= longest {
                Photo::Camera(camera)
            } else {
                let pixels = grainy_codecs::resize_float(camera.pixels(), longest).map_err(fail)?;
                let info = camera.raw_info().clone();
                Photo::Camera(Box::new(CameraSource::new(pixels, info).map_err(|e| failure("ValueError", e))?))
            }
        }
    })
}

/// What one photo's computation gives.
enum AutoValue {
    /// The new settings.
    Edit(Settings),
    /// A negative's sample for a roll measured together.
    Sample(Array2<f32>),
}

/// The engine behind `photo_service.auto_values`: the 720-pixel source of a photo (or its offline preview) and
/// the computation of the kind.
struct Engine {
    kind: AutoKind,
    amount: f64,
    directory: PathBuf,
}

impl AutoSource for Engine {
    type Image = Photo;
    type Value = AutoValue;

    fn load(&mut self, record: &Value, path: &Path) -> Result<Photo, Failure> {
        let loaded = load_photo(path, Some(720), &settings_of(record)).map_err(|e| files_failure(&e))?;
        Ok(loaded.photo)
    }

    fn offline(&mut self, record: &Value) -> Result<Photo, Failure> {
        offline(&self.directory, id_of(record), &settings_of(record))
    }

    fn compute(&mut self, image: Photo, settings: &Value) -> Result<AutoValue, Failure> {
        let photo = reduced(image, 720)?;
        let s = Settings(settings.as_object().cloned().unwrap_or_default());
        let externals = Externals::default();
        Ok(match self.kind {
            AutoKind::Tone => AutoValue::Edit(
                grainy_engine::auto::auto_tone_settings(photo.source(), &s, &externals)
                    .map_err(|e| auto_failure(&e))?
                    .0,
            ),
            AutoKind::Color => AutoValue::Edit(
                grainy_engine::auto::auto_color_settings(photo.source(), &s, self.amount, &externals)
                    .map_err(|e| auto_failure(&e))?
                    .0,
            ),
            AutoKind::Negative => AutoValue::Edit(
                grainy_engine::auto::negative_converted(photo.source(), &s, &externals)
                    .map_err(|e| auto_failure(&e))?,
            ),
            AutoKind::Roll => AutoValue::Sample(
                grainy_engine::auto::negative_sample(photo.source(), &s, &externals).map_err(|e| auto_failure(&e))?,
            ),
        })
    }
}

/// What the work of a batch hands to its finish on the window's thread.
enum Outcome {
    /// Cancelled while computing.
    Cancelled,
    /// The new settings of every photo, in the order of the records.
    Values(Vec<Settings>),
    /// The scan-border crop of every photo (`None`: no film edge found).
    Crops(Vec<Option<[f64; 4]>>),
}

fn directory(cx: &AppContext) -> PathBuf {
    cx.doc.metadata_catalog().map(|c| c.directory().to_path_buf()).unwrap_or_default()
}

/// The oracle's progress box over the job `id`.
fn open_progress(cx: &mut AppContext, id: crate::jobs::JobId, title: String, text: String, shared: &Shared) {
    cx.open_dialog(Box::new(ProgressBox::new(id, title, text, shared.clone())));
}

// ---- automatic tone, colour, negatives -------------------------------------------------------------------------------

/// One of the batches of `MainWindow.command_auto_values`.
struct Auto {
    kind: AutoKind,
    /// `name`: what the cancel message and the box call it.
    name: &'static str,
    /// The history label and the status line's name.
    label: &'static str,
}

/// `command_auto_values(records, compute, name)` and then `apply_edits`: every photo is computed first; a cancel or
/// a failure writes nothing.
fn start_auto(cx: &mut AppContext, auto: Auto, records: Vec<Value>) {
    let amount = cx.doc.preference("auto_color_amount").and_then(|v| v.as_f64()).unwrap_or(100.0) / 100.0;
    let load = auto_records(auto.kind, &records).unwrap_or_else(|_| records.clone());
    let directory = directory(cx);
    let total = records.len();
    let shared = Shared::new(total);
    let (work_shared, done_shared) = (shared.clone(), shared.clone());
    let originals: Vec<Settings> = records.iter().map(settings_of).collect();
    let kind = auto.kind;
    let (name, label) = (auto.name, auto.label);
    let cancel_first = CANCEL_NEXT.swap(false, std::sync::atomic::Ordering::SeqCst);
    let generation = cx.document_generation;
    let job = cx.jobs.spawn(
        KIND,
        tr(label),
        move |handle| {
            if cancel_first {
                handle.cancel_flag().store(true, std::sync::atomic::Ordering::SeqCst);
            }
            let mut engine = Engine { kind, amount, directory };
            let mut progress = |n: usize| {
                work_shared.set_done(n);
                handle.progress(n, total, tr(label));
            };
            let (values, _) = auto_values(&load, &mut engine, Some(handle.cancel_flag()), Some(&mut progress), true)
                .map_err(|e| e.to_string())?;
            let Some(values) = values else { return Ok(Outcome::Cancelled) };
            let mut edits = Vec::new();
            let mut samples = Vec::new();
            for value in values.into_iter().flatten() {
                match value {
                    AutoValue::Edit(settings) => edits.push(settings),
                    AutoValue::Sample(sample) => samples.push(sample),
                }
            }
            if kind == AutoKind::Roll {
                return Ok(Outcome::Values(grainy_engine::auto::negative_converted_together(&samples, &originals)));
            }
            Ok(Outcome::Values(edits))
        },
        move |cx: &mut AppContext, result: Result<Outcome, String>| {
            done_shared.finish();
            if generation != cx.document_generation {
                return;
            }
            match result {
                Ok(Outcome::Values(values)) => {
                    commit_pending(cx);
                    apply_or_fail(cx, &records, values, auto.label);
                }
                Ok(_) => failed(cx, &format!("일괄 {name}을 취소했습니다. 변경한 사진은 없습니다.")),
                Err(_) => failed(cx, "일부 사진을 읽거나 계산할 수 없습니다. 변경한 사진은 없습니다."),
            }
        },
    );
    let title = if name == "자동톤" { tr("일괄 자동톤") } else { tr(name) };
    open_progress(cx, job, title, format!("사진별 {name}을 계산하는 중…"), &shared);
}

/// Port of `auto_tone`: the develop panel's auto tone for every selected photo.
fn auto_tone(cx: &mut AppContext, _: &Invocation) {
    let records = records(cx);
    if !records.is_empty() {
        start_auto(cx, Auto { kind: AutoKind::Tone, name: "자동톤", label: "자동톤" }, records);
    }
}

/// Port of `auto_color`: at the saved strength (`auto_color_amount`).
fn auto_color(cx: &mut AppContext, _: &Invocation) {
    let records = records(cx);
    if !records.is_empty() {
        start_auto(cx, Auto { kind: AutoKind::Color, name: "자동 색", label: "자동 색" }, records);
    }
}

/// Port of `invert_negatives`: each photo measured on its own.
fn invert_negatives(cx: &mut AppContext, _: &Invocation) {
    let records = records(cx);
    if !records.is_empty() {
        start_auto(cx, Auto { kind: AutoKind::Negative, name: "네거티브 반전", label: "네거티브 반전" }, records);
    }
}

/// Port of `invert_roll`: one conversion for all selected photos.
fn invert_roll(cx: &mut AppContext, _: &Invocation) {
    let records = records(cx);
    if records.len() < 2 {
        cx.status = tr("한 롤로 맞추려면 같은 롤의 사진을 두 장 이상 선택하세요.");
        return;
    }
    start_auto(cx, Auto { kind: AutoKind::Roll, name: "네거티브 반전", label: "네거티브 반전 (한 롤)" }, records);
}

// ---- white balance and scan borders (their own jobs) -------------------------------------------------------------------

/// `Failure`-style text of an error of the engine: `ValueError: …`, the last line of a Python traceback.
fn traceback_line(kind: &str, message: &str) -> String {
    format!("{kind}: {message}")
}

/// What a batch with its own progress box needs: the computation per photo.
fn spawn_own(
    cx: &mut AppContext,
    records: Vec<Value>,
    label: &'static str,
    text: &'static str,
    counted: bool,
    compute: impl Fn(&Value, &Settings) -> Result<Computed, String> + Send + 'static,
    finish: impl FnOnce(&mut AppContext, Vec<Value>, Outcome) + 'static,
) {
    let total = records.len();
    let shared = Shared::new(if counted { total } else { 0 });
    let (work_shared, done_shared) = (shared.clone(), shared.clone());
    let work_records = records.clone();
    let generation = cx.document_generation;
    let job = cx.jobs.spawn(
        KIND,
        tr(label),
        move |handle| {
            let mut values = Vec::new();
            for (index, record) in work_records.iter().enumerate() {
                if handle.cancelled() {
                    return Ok(Outcome::Cancelled);
                }
                values.push(compute(record, &settings_of(record))?);
                work_shared.set_done(index + 1);
                handle.progress(index + 1, total, tr(label));
            }
            Ok(match values.first() {
                Some(Computed::Crop(_)) => Outcome::Crops(
                    values
                        .into_iter()
                        .map(|v| match v {
                            Computed::Crop(c) => c,
                            Computed::Edit(_) => None,
                        })
                        .collect(),
                ),
                _ => Outcome::Values(
                    values
                        .into_iter()
                        .filter_map(|v| match v {
                            Computed::Edit(s) => Some(s),
                            Computed::Crop(_) => None,
                        })
                        .collect(),
                ),
            })
        },
        move |cx: &mut AppContext, result: Result<Outcome, String>| {
            done_shared.finish();
            if generation != cx.document_generation {
                return;
            }
            match result {
                Ok(Outcome::Cancelled) => {}
                Ok(outcome) => finish(cx, records, outcome),
                Err(message) => error_box(cx, &message),
            }
        },
    );
    open_progress(cx, job, tr(text), tr(text), &shared);
}

/// One photo's result in a batch with its own box.
enum Computed {
    Edit(Settings),
    Crop(Option<[f64; 4]>),
}

/// Port of `auto_white`: the white balance of every selected photo from a 640-pixel reading.
fn auto_white(cx: &mut AppContext, _: &Invocation) {
    let records = records(cx);
    if records.is_empty() {
        return;
    }
    spawn_own(
        cx,
        records,
        "자동 화이트 밸런스",
        "자동 화이트 밸런스 계산 중…",
        false,
        |record, settings| {
            let path = record["path"].as_str().unwrap_or("");
            let loaded = load_photo(path, Some(640), settings).map_err(|e| {
                let f = files_failure(&e);
                traceback_line(&f.kind, &f.message)
            })?;
            grainy_engine::auto::automatic_white_balance(loaded.photo.source(), settings, &Externals::default())
                .map(Computed::Edit)
                .map_err(|e| traceback_line(&auto_failure(&e).kind, &e.to_string()))
        },
        |cx, records, outcome| {
            if let Outcome::Values(values) = outcome {
                // Includes edits made while the computation was in flight.
                commit_pending(cx);
                apply_or_fail(cx, &records, values, "자동 화이트 밸런스");
            }
        },
    );
}

/// Port of `border_crop`: `scan_border.crop_for` on every selected photo; photos without a film edge stay.
fn border_crop(cx: &mut AppContext, _: &Invocation) {
    let records = records(cx);
    if records.is_empty() {
        return;
    }
    spawn_own(
        cx,
        records,
        "스캔 테두리 자동 자르기",
        "스캔 테두리 분석 중…",
        true,
        |record, settings| {
            let path = record["path"].as_str().unwrap_or("");
            let loaded = load_photo(path, Some(1200), settings).map_err(|e| {
                let f = files_failure(&e);
                traceback_line(&f.kind, &f.message)
            })?;
            grainy_engine::auto::scan_border_crop(loaded.photo.source(), settings, 1200, &Externals::default())
                .map(Computed::Crop)
                .map_err(|e| traceback_line(&auto_failure(&e).kind, &e.to_string()))
        },
        |cx, records, outcome| {
            let Outcome::Crops(crops) = outcome else { return };
            let found: Vec<(Value, Settings)> = records
                .iter()
                .zip(&crops)
                .filter_map(|(record, crop)| {
                    let crop = (*crop)?;
                    let mut settings = settings_of(record);
                    settings.0.insert("crop".into(), json!(crop));
                    Some((record.clone(), settings))
                })
                .collect();
            commit_pending(cx);
            let count = found.len();
            if !found.is_empty() {
                let (rows, values): (Vec<Value>, Vec<Settings>) = found.into_iter().unzip();
                apply_or_fail(cx, &rows, values, "스캔 테두리 자동 자르기");
            }
            cx.status = tr_with(
                "스캔 테두리: {0}장 자름 · {1}장은 테두리를 찾지 못함",
                &[&thousands(count), &thousands(records.len() - count)],
            );
        },
    );
}

// ---- process update and exposure ------------------------------------------------------------------------------------------

/// Port of `update_process`: the selected photos on the current processes (tone and detail version 3).
fn update_process(cx: &mut AppContext, _: &Invocation) {
    let records: Vec<Value> = records(cx)
        .into_iter()
        .filter(|r| {
            let settings = settings_of(r);
            let normal = grainy_core::normalized(&settings.0);
            let detail = r["settings"].get("detail_version").and_then(Value::as_f64).unwrap_or(3.0);
            normal.f64("tone_version") < 3.0 || detail < 3.0
        })
        .collect();
    if records.is_empty() {
        cx.status = tr("선택한 사진은 이미 최신 처리 방식입니다.");
        return;
    }
    let values = records
        .iter()
        .map(|r| {
            let mut settings = settings_of(r);
            settings.0.insert("tone_version".into(), json!(3));
            settings.0.insert("detail_version".into(), json!(3));
            settings
        })
        .collect();
    apply_or_fail(cx, &records, values, "최신 처리 방식으로 업데이트");
}

/// Port of `match_exposure`: the selected photos' exposure set so that their total exposure equals the open
/// photo's.
fn match_exposure(cx: &mut AppContext, _: &Invocation) {
    let records = records(cx);
    let Some(reference) = row(cx, cx.doc.current_id() as i64) else { return };
    let targets: Vec<Value> = records.into_iter().filter(|r| id_of(r) != id_of(&reference)).collect();
    if targets.is_empty() {
        failed(cx, "기준 사진과 맞출 사진을 함께 선택하세요.");
        return;
    }
    let mut values = Vec::new();
    for target in &targets {
        match grainy_library::workflow_data::match_exposure(&reference, target) {
            Ok(settings) => values.push(settings),
            Err(error) => return failed(cx, &error.to_string()),
        }
    }
    apply_or_fail(cx, &targets, values, "총노출 맞추기");
}

// ---- metadata presets ---------------------------------------------------------------------------------------------------

/// The eleven metadata fields of a preset in the oracle's order (`METADATA_KEYS`) with their titles.
pub const FIELDS: [(&str, &str); 11] = [
    ("title", "제목"),
    ("caption", "설명"),
    ("creator", "촬영자"),
    ("copyright", "저작권"),
    ("location", "장소"),
    ("city", "도시"),
    ("country", "국가"),
    ("latitude", "위도"),
    ("longitude", "경도"),
    ("DateTimeOriginal", "촬영 시각"),
    ("keywords", "키워드"),
];

/// What a chosen preset is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// 메타데이터 프리셋 적용.
    Apply,
    /// 메타데이터 프리셋 삭제.
    Delete,
}

/// What follows the keyword box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Then {
    /// 바로가기 키워드 설정: only keep the keyword.
    Set,
    /// 바로가기 키워드 적용 without a keyword yet: add it to the photos afterwards.
    Apply,
}

/// Port of `save_metadata`: the form for the open photo.
fn metadata_save(cx: &mut AppContext, _: &Invocation) {
    let Some(photo) = row(cx, cx.doc.current_id() as i64) else { return };
    cx.close_dialog("workflow_preset_save");
    cx.open_dialog(Box::new(PresetSave::new(&photo)));
}

/// What `data.save_metadata_preset` refuses, with its message: the name, the GPS ranges and the capture time.
fn preset_error(name: &str, values: &Map<String, Value>) -> Option<String> {
    if name.trim().is_empty() {
        return Some("프리셋 이름을 입력하세요.".into());
    }
    for (key, limit) in [("latitude", 90.0), ("longitude", 180.0)] {
        let Some(text) = values.get(key).and_then(Value::as_str).filter(|t| !t.is_empty()) else { continue };
        let number: f64 = match text.trim().parse() {
            Ok(number) => number,
            Err(_) => return Some(format!("could not convert string to float: {}", python_repr(text))),
        };
        if !number.is_finite() || number.abs() > limit {
            return Some("GPS 좌표 범위를 확인하세요.".into());
        }
    }
    if let Some(text) = values.get("DateTimeOriginal").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        if let Err(message) = parse_stamp(text) {
            return Some(message);
        }
    }
    None
}

/// The form's 저장: the ticked fields become the preset `name`.
pub fn save_preset_done(cx: &mut AppContext, name: &str, rows: &[Row]) {
    let values: Map<String, Value> =
        rows.iter().filter(|r| r.checked).map(|r| (r.key.to_owned(), Value::String(r.text.clone()))).collect();
    if values.is_empty() {
        return failed(cx, "저장할 메타데이터 항목을 선택하세요.");
    }
    if let Some(message) = preset_error(name, &values) {
        return failed(cx, &message);
    }
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    if let Err(error) = grainy_catalog::workflow::save_metadata_preset(catalog, name, &Value::Object(values)) {
        failed(cx, &error.to_string());
    }
}

/// The names of the saved presets, in the order the list shows them.
pub fn preset_names(cx: &AppContext) -> Vec<String> {
    presets(cx).keys().cloned().collect()
}

/// The saved presets (`data.metadata_presets`).
fn presets(cx: &AppContext) -> Map<String, Value> {
    cx.doc
        .metadata_catalog()
        .and_then(|c| grainy_catalog::workflow::metadata_presets(c).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// Port of `choose_metadata`: the list of the saved presets, then `what` with the choice.
fn choose(cx: &mut AppContext, what: Choice) {
    let names: Vec<String> = presets(cx).keys().cloned().collect();
    if names.is_empty() {
        return failed(cx, "저장된 메타데이터 프리셋이 없습니다.");
    }
    cx.close_dialog("workflow_preset_choose");
    cx.open_dialog(Box::new(PresetChoose::new(what, names)));
}

fn metadata_apply(cx: &mut AppContext, _: &Invocation) {
    choose(cx, Choice::Apply);
}

fn metadata_delete(cx: &mut AppContext, _: &Invocation) {
    choose(cx, Choice::Delete);
}

/// The list's 확인: apply the preset `name` to the selected photos, or delete it.
pub fn choose_done(cx: &mut AppContext, what: Choice, name: &str) {
    let mut all = presets(cx);
    match what {
        Choice::Apply => {
            let Some(values) = all.get(name).cloned() else { return };
            let ids = selected_ids(cx);
            let Some(catalog) = cx.doc.metadata_catalog() else { return };
            match grainy_catalog::workflow::apply_metadata_preset(catalog, &ids, &values) {
                Ok(()) => metadata::changed(cx),
                Err(error) => failed(cx, &error.to_string()),
            }
        }
        Choice::Delete => {
            all.remove(name);
            if let Err(message) = cx.doc.save_preference("metadata_presets", &Value::Object(all)) {
                failed(cx, &message);
            }
        }
    }
}

// ---- the shortcut keyword ------------------------------------------------------------------------------------------------

/// The saved shortcut keyword (`shortcut_keyword`), empty when none is saved.
pub fn saved_keyword(cx: &AppContext) -> String {
    cx.doc.preference("shortcut_keyword").as_ref().and_then(Value::as_str).unwrap_or("").to_owned()
}

/// Port of `set_keyword`: the box with the saved keyword.
fn keyword_set(cx: &mut AppContext, _: &Invocation) {
    let text = saved_keyword(cx);
    cx.close_dialog("workflow_keyword");
    cx.open_dialog(Box::new(KeywordInput::new(text, Then::Set)));
}

/// The box's 확인: one keyword, no comma; it is kept, and added to the photos when it was asked for by 적용.
pub fn keyword_done(cx: &mut AppContext, then: Then, text: &str) {
    if text.contains(',') || text.trim().is_empty() {
        return failed(cx, "키워드 하나를 입력하세요.");
    }
    if let Err(message) = cx.doc.save_preference("shortcut_keyword", &Value::String(text.trim().to_owned())) {
        return failed(cx, &message);
    }
    if then == Then::Apply {
        add_saved_keyword(cx);
    }
}

/// Port of `apply_keyword`: the saved keyword (asked for first when there is none) added to the selected photos.
fn keyword_apply(cx: &mut AppContext, _: &Invocation) {
    if saved_keyword(cx).is_empty() {
        cx.close_dialog("workflow_keyword");
        cx.open_dialog(Box::new(KeywordInput::new(String::new(), Then::Apply)));
        return;
    }
    add_saved_keyword(cx);
}

fn add_saved_keyword(cx: &mut AppContext) {
    let keyword = saved_keyword(cx);
    if keyword.is_empty() {
        return;
    }
    let ids = selected_ids(cx);
    if keyword.trim().is_empty() || keyword.contains(',') {
        return failed(cx, "키워드 하나를 입력하세요. 계층은 |로 구분합니다.");
    }
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    match grainy_catalog::workflow::add_keyword(catalog, &ids, &keyword) {
        Ok(()) => {
            metadata::changed(cx);
            cx.status = tr_with("키워드 “{0}”를 {1}장에 추가", &[&keyword, &thousands(ids.len())]);
        }
        Err(error) => failed(cx, &error.to_string()),
    }
}
