//! 내보내기: the export dialog, the quick exports (작은 크기 / 원본 크기 / 원본 + 설정 / 이전 설정으로 내보내기, Ctrl+E),
//! the export job with the catalogue's history and the failure list, 내보내기 기록… and 내보내기 상태 초기화
//! (luma/app.py `export_dialog`, `quick_export`, `start_export`; luma/quick_export.py `preset`, `previous`;
//! luma/workflow_data.py `record_exports`, `export_history`; luma/workflow_ui.py `show_exports`, `reset_exports`).

use std::path::{Path, PathBuf};

use grainy_catalog::Catalog;
use serde_json::{json, Map, Value};

use super::{always, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::export::{ExportDialog, ExportFailures, QuickFolder};
use crate::dialogs::export_history::ExportHistory;
use crate::export::{self, ExportOptions, Outcome};
use crate::text::{tr, tr_with};

/// The preference holding the export records (`workflow_data.record_exports`).
pub const HISTORY: &str = "export_history";

pub const COMMANDS: &[Command] = &[
    Command { id: "file.export", label: "내보내기…", enabled: not_exporting, run: export },
    Command {
        id: "file.quick_export", label: "이전 설정으로 내보내기", enabled: can_quick, run: quick_previous
    },
    Command { id: "export.quick_small", label: "JPG (작은 크기 · 2048px)", enabled: can_quick, run: quick_small },
    Command { id: "export.quick_large", label: "JPG (원본 크기)", enabled: can_quick, run: quick_large },
    Command { id: "export.quick_original", label: "원본 + 설정", enabled: can_quick, run: quick_original },
    Command { id: "export.history", label: "내보내기 기록…", enabled: always, run: show_history },
    Command { id: "export.reset_history", label: "내보내기 상태 초기화", enabled: always, run: reset_history },
];

fn not_exporting(cx: &AppContext) -> bool {
    !cx.jobs.running("export")
}

/// The quick exports need photos and no export running (`quick_export`'s first lines).
fn can_quick(cx: &AppContext) -> bool {
    !cx.jobs.running("export") && !cx.doc.is_empty()
}

/// The ids the oracle's `selected_ids()` gives: the selected photos (the open one when none is selected).
pub fn selected_ids(cx: &AppContext) -> Vec<u64> {
    let photos = cx.doc.photos();
    cx.doc.selected_indices().iter().filter_map(|i| photos.get(*i)).map(|p| p.id).collect()
}

// ---- the saved settings ---------------------------------------------------------------------------------------

/// Port of `quick_export.previous(self.catalog)`: the settings of the last export of this library when they are
/// valid. A session has no catalogue; it keeps what its last export used (`cx.export_options`).
pub fn saved(cx: &AppContext) -> Option<Map<String, Value>> {
    if cx.doc.metadata_catalog().is_some() {
        let stored = cx.doc.preference(grainy_library::quick_export::PREFERENCE);
        return export::previous(stored.as_ref());
    }
    let memory = Value::Object(cx.export_options.saved());
    export::previous(Some(&memory))
}

/// Whether "이전 설정으로 내보내기" has settings to use (the photo menu greys it out otherwise).
pub fn previous_available(cx: &AppContext) -> bool {
    saved(cx).is_some()
}

// ---- the export dialog ----------------------------------------------------------------------------------------

/// The dialog's values when it opens: the previous export's (`ExportDialog.__init__` reads
/// `quick_export.previous`), the defaults otherwise.
pub fn dialog_options(cx: &AppContext) -> ExportOptions {
    saved(cx).map_or_else(ExportOptions::default, |map| {
        let mut options = ExportOptions::from_saved(&map);
        // The format list's `format_changed` runs after the values are set: JPEG XL is always sRGB.
        options.set_format(options.format);
        options
    })
}

/// Port of `export_dialog`: the stored edits are committed, then the box for the selected photos opens.
fn export(cx: &mut AppContext, _: &Invocation) {
    if cx.doc.is_empty() {
        cx.status = tr("내보낼 사진이 없습니다.");
        return;
    }
    commit(cx);
    let count = selected_ids(cx).len();
    let options = dialog_options(cx);
    cx.open_dialog(Box::new(ExportDialog { options, message: None, count }));
}

/// `self.commit()`: the open photo's pending change is one undo step and the journal is stored.
fn commit(cx: &mut AppContext) {
    cx.doc.edit_mut().commit("보정 변경");
    if let Err(message) = cx.doc.flush() {
        cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
    }
}

// ---- the quick exports ----------------------------------------------------------------------------------------

/// What `quick_export(kind)` does next.
#[derive(Debug, Clone, PartialEq)]
pub enum Quick {
    /// Nothing: an export is running, or no photo is selected.
    Nothing,
    /// "previous" without saved settings opens the export dialog.
    Dialog,
    /// Export now with these values (the folder is the saved one and exists).
    Start(ExportOptions),
    /// A folder has to be asked for first (`QFileDialog.getExistingDirectory(title, start)`); the answer completes
    /// `options`.
    Ask { title: &'static str, start: String, options: ExportOptions },
}

/// Port of `quick_export`'s decisions up to the folder dialog. `kind` is `small`, `large`, `original` or
/// `previous`.
pub fn plan_quick(cx: &mut AppContext, kind: &str) -> Quick {
    if cx.jobs.running("export") {
        return Quick::Nothing;
    }
    commit(cx);
    if selected_ids(cx).is_empty() {
        return Quick::Nothing;
    }
    let saved = saved(cx);
    if kind == "previous" {
        let Some(saved) = saved else { return Quick::Dialog };
        let options = ExportOptions::from_saved(&saved);
        if Path::new(&options.folder).is_dir() {
            return Quick::Start(options);
        }
        let start = options.folder.clone();
        return Quick::Ask { title: "이전 저장 위치가 없습니다. 내보낼 폴더 선택", start, options };
    }
    let Ok(preset) = grainy_library::quick_export::preset(kind) else { return Quick::Nothing };
    let start = saved.as_ref().and_then(|s| s["folder"].as_str()).unwrap_or_default().to_owned();
    Quick::Ask { title: "내보낼 폴더 선택", start, options: ExportOptions::from_saved(&preset) }
}

/// Runs a plan: starts the export, opens the dialog or asks for the folder.
pub fn run_quick(cx: &mut AppContext, plan: Quick) {
    match plan {
        Quick::Nothing => {}
        Quick::Dialog => export(cx, &Invocation::default()),
        Quick::Start(options) => start_export(cx, options),
        Quick::Ask { title, start, options } => cx.open_dialog(Box::new(QuickFolder::new(cx, title, start, options))),
    }
}

/// The folder answer of the quick export: `if folder: self.start_export(ids, Path(folder), **options)`.
pub fn finish_quick(cx: &mut AppContext, mut options: ExportOptions, folder: Option<PathBuf>) {
    if let Some(folder) = folder.filter(|f| !f.as_os_str().is_empty()) {
        options.folder = folder.to_string_lossy().into_owned();
        start_export(cx, options);
    }
}

fn quick(cx: &mut AppContext, kind: &str) {
    let plan = plan_quick(cx, kind);
    run_quick(cx, plan);
}

fn quick_previous(cx: &mut AppContext, _: &Invocation) {
    quick(cx, "previous");
}

fn quick_small(cx: &mut AppContext, _: &Invocation) {
    quick(cx, "small");
}

fn quick_large(cx: &mut AppContext, _: &Invocation) {
    quick(cx, "large");
}

fn quick_original(cx: &mut AppContext, _: &Invocation) {
    quick(cx, "original");
}

// ---- the export job -------------------------------------------------------------------------------------------

/// Port of `start_export`: the selected photos with their catalogue rows, exported on a background job; existing
/// files are never overwritten. When files were written the options become the "previous export" and the files
/// are recorded in the export history; a message lists the photos that failed.
pub fn start_export(cx: &mut AppContext, options: ExportOptions) {
    if cx.jobs.running("export") {
        return;
    }
    let ids = selected_ids(cx);
    if ids.is_empty() {
        return;
    }
    commit(cx);
    if !Path::new(&options.folder).is_dir() {
        cx.status = tr_with("처리하지 못했습니다: {0}", &[&tr("저장할 폴더를 선택해 주세요.")]);
        return;
    }
    let settings: Vec<grainy_core::Settings> = cx
        .doc
        .selected_indices()
        .into_iter()
        .filter(|i| cx.doc.photos().get(*i).is_some())
        .map(|i| cx.doc.settings_of(i))
        .collect();
    let catalog_folder = cx.doc.metadata_catalog().map(|c| c.directory().to_path_buf());
    let library = cx.doc.library_folder();
    let session: Vec<Value> = if catalog_folder.is_none() {
        ids.iter()
            .zip(&settings)
            .filter_map(|(id, settings)| {
                let photo = cx.doc.photos().iter().find(|p| p.id == *id)?;
                Some(export::session_record(*id, &photo.path, settings, &cx.doc.photo_info(*id)))
            })
            .collect()
    } else {
        Vec::new()
    };
    cx.status = tr_with("사진 {0}장 내보내는 중…", &[&ids.len().to_string()]);
    let work_options = options.clone();
    cx.jobs.spawn(
        "export",
        tr("내보내기"),
        move |handle| {
            let records = match &catalog_folder {
                Some(folder) => {
                    let reader = Catalog::open_reader(folder).map_err(|e| e.to_string())?;
                    export::catalog_records(&reader, &ids, &settings)?
                }
                None => session,
            };
            let progress = |done: usize, total: usize| {
                handle.progress(done, total, tr_with("내보내는 중… {0}/{1}", &[&done.to_string(), &total.to_string()]));
            };
            export::run_records(&records, &work_options, &progress, handle.cancel_flag())
        },
        move |cx: &mut AppContext, result: Result<Outcome, String>| finished(cx, &options, library, result),
    );
}

/// `finished(result)` of `start_export`.
fn finished(cx: &mut AppContext, options: &ExportOptions, library: Option<PathBuf>, result: Result<Outcome, String>) {
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(message) => {
            cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
            return;
        }
    };
    let mut error = None;
    if !outcome.outputs.is_empty() {
        cx.export_options = options.clone();
        // The library the export started in is still the one open: its records and "previous export" are saved.
        if cx.doc.library_folder() == library && !cx.doc.read_only() {
            if let Err(message) = remember(cx, options, &outcome) {
                error = Some(message);
            }
        }
    }
    cx.status = match error {
        Some(message) => tr_with("처리하지 못했습니다: {0}", &[&message]),
        None => tr_with(
            "내보내기 완료: {0}장  ·  실패: {1}장",
            &[&outcome.outputs.len().to_string(), &outcome.failures.len().to_string()],
        ),
    };
    if !outcome.failures.is_empty() {
        cx.open_dialog(Box::new(ExportFailures { lines: outcome.failures }));
    }
}

/// `save_preference(PREFERENCE, options)` and `record_exports(catalog, outputs)` of the finished export.
fn remember(cx: &mut AppContext, options: &ExportOptions, outcome: &Outcome) -> Result<(), String> {
    cx.doc.save_preference(grainy_library::quick_export::PREFERENCE, &Value::Object(options.saved()))?;
    if let Some(catalog) = cx.doc.metadata_catalog() {
        grainy_catalog::workflow::record_exports(catalog, &outcome.history_items()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---- the export history ---------------------------------------------------------------------------------------

/// One line of 내보내기 기록….
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRow {
    /// The photo's name in the catalogue.
    pub name: String,
    /// When the file was written (the record's `created`).
    pub created: String,
    /// The state text: 출력 파일 없음 when the file is gone, then 내보낸 뒤 보정 변경 / 현재 보정과 일치.
    pub status: String,
    /// The file written.
    pub path: String,
}

/// Port of `show_exports`' rows: `workflow_data.export_history` for `ids`, newest first.
pub fn history_rows(catalog: &Catalog, ids: &[u64]) -> Result<Vec<HistoryRow>, String> {
    let ids: Vec<i64> = ids.iter().map(|i| *i as i64).collect();
    let records = grainy_catalog::workflow::export_history(catalog, &ids).map_err(|e| e.to_string())?;
    let mut rows = Vec::with_capacity(records.len());
    for record in records {
        let mut status = if record["edited_since"].as_bool().unwrap_or(false) {
            tr("내보낸 뒤 보정 변경")
        } else {
            tr("현재 보정과 일치")
        };
        let path = record["path"].as_str().unwrap_or_default().to_owned();
        if !Path::new(&path).is_file() {
            status = format!("{} · {status}", tr("출력 파일 없음"));
        }
        let name = record["photo_id"]
            .as_i64()
            .and_then(|id| catalog.photo(id).ok().flatten())
            .and_then(|photo| photo["name"].as_str().map(str::to_owned))
            .unwrap_or_default();
        rows.push(HistoryRow {
            name,
            created: record["created"].as_str().unwrap_or_default().to_owned(),
            status,
            path,
        });
    }
    Ok(rows)
}

/// Port of `show_exports`: the box with the export records of the selected photos.
fn show_history(cx: &mut AppContext, _: &Invocation) {
    commit(cx);
    let ids = selected_ids(cx);
    let rows = match cx.doc.metadata_catalog() {
        Some(catalog) => history_rows(catalog, &ids).unwrap_or_else(|message| {
            cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
            Vec::new()
        }),
        None => Vec::new(),
    };
    cx.open_dialog(Box::new(ExportHistory::new(rows)));
}

/// Port of `reset_exports`: the selected photos' export records are forgotten (so "내보낸 뒤 보정 변경" starts
/// over); the other photos' records stay.
fn reset_history(cx: &mut AppContext, _: &Invocation) {
    let ids = selected_ids(cx);
    if let Err(message) = forget(cx, &ids) {
        cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
    }
}

/// The records of photos other than `ids` stay in the export history preference.
pub fn forget(cx: &mut AppContext, ids: &[u64]) -> Result<(), String> {
    let values = cx.doc.preference(HISTORY).unwrap_or_else(|| json!([]));
    let kept: Vec<Value> = values
        .as_array()
        .map(|list| {
            list.iter()
                .filter(|r| !r.get("photo_id").and_then(Value::as_u64).is_some_and(|id| ids.contains(&id)))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    cx.doc.save_preference(HISTORY, &Value::Array(kept))
}
