//! 추가 도구 → Lightroom 사진·컬렉션 가져오기 and 최근 Lightroom 가져오기 결과 (work package 29): luma/extras.py
//! `import_lightroom` / `lightroom_report`, luma/lightroom_import_dialog.py `begin_import`, and the importer
//! itself ([`import`], a port of luma/lightroom_import.py, with [`history`], a port of
//! `adobe_history.transfer`).
//!
//! The flow is the oracle's: the open photo's change is stored, the maintenance window opens (Lightroom 가져오기),
//! a job inspects the catalogue, the review box (Lightroom 가져오기 범위: what is imported, the 변환 check box, 가져오기
//! / 취소) asks, then a second job imports with progress in the maintenance window, and the result box lists
//! what was added and what needs checking. 취소 in the maintenance window stops either job at its next safe point;
//! a cancelled import leaves no record behind. The result of every import is stored in the library
//! (`lightroom_import_runs`), which 최근 Lightroom 가져오기 결과 shows again.
//!
//! **Data safety.** The `.lrcat` is opened read-only and never changed; photo files are never opened (only their
//! paths are recorded); the library is copied to `backups/` before anything is written, and the whole import is
//! one transaction.

pub mod history;
pub mod import;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use super::{Command, Invocation};
use crate::commands::files::FilesState;
use crate::context::AppContext;
use crate::dialogs::about::MessageBox;
use crate::dialogs::import_errors::Notice;
use crate::dialogs::lightroom::ReviewDialog;
use crate::dialogs::maintenance::{MaintenanceDialog, PathWait, Pick, Shared, Update};
use crate::document::catalog::files::FileOp;
use crate::text::tr;
use import::{Clock, ImportResult, Inspection, SystemClock};

pub const COMMANDS: &[Command] = &[
    Command {
        id: "extras.lightroom_import",
        label: "Lightroom 사진·컬렉션 가져오기",
        enabled: writable,
        run: import_lightroom,
    },
    Command {
        id: "extras.lightroom_report",
        label: "최근 Lightroom 가져오기 결과",
        enabled: has_catalog,
        run: lightroom_report,
    },
];

fn writable(cx: &AppContext) -> bool {
    cx.doc.library_folder().is_some()
}

fn has_catalog(cx: &AppContext) -> bool {
    cx.doc.metadata_catalog().is_some()
}

// ---- texts --------------------------------------------------------------------------------------------------

/// English for the oracle's texts its `en.json` lacks (the Python Grainy shows them in Korean).
const ENGLISH: &[(&str, &str)] = &[
    (
        "사진과 분류를 가져옵니다. 기존 Luma 사진은 별도 사본으로 보존합니다.\n지원하는 스마트 규칙을 연결하고, 보정값·이력 변환 여부를 다음 화면에서 선택합니다.",
        "Photos and their labels are imported. Photos already in the library are kept, and changed ones come in as separate copies.\nSupported smart rules are linked; whether edits and history are converted is chosen on the next screen.",
    ),
    (
        "사진과 가상 사본 {0}개를 확인했습니다.\n\n별점·선택 표시·색 라벨·계층 키워드·IPTC·컬렉션과 폴더 스택을 가져옵니다.\n기존 Luma 보정과 분류를 덮어쓰지 않으며, 변경된 Lightroom 기록은 새 사본으로 가져옵니다.\n\n지원하는 스마트 컬렉션은 사진의 현재 분류·메타데이터로 검색합니다. 지원하지 않는 조건이 포함되면 전체 규칙을 보관하고 일반 컬렉션으로 가져옵니다.\n\n아래 선택을 켜면 지원하는 보정값·이력·스냅샷도 변환합니다. Luma와 Adobe의 현상 방식이 달라 같은 모습은 보장되지 않습니다. 변환하지 못한 항목은 결과에 표시하고 원문을 보관합니다.\n\n누락된 원본은 참조 경로로 등록됩니다. 가져오기를 진행할까요?",
        "Found {0} photos and virtual copies.\n\nRatings, picks, colour labels, keyword hierarchies, IPTC, collections and folder stacks are imported.\nEdits and labels already in the library are not overwritten; Lightroom records that changed come in as new copies.\n\nSupported smart collections search the photos' current labels and metadata. When a rule has conditions that are not supported, the whole rule is kept and the collection comes in as a regular one.\n\nTurn on the option below to convert supported edits, history and snapshots as well. Grainy and Adobe develop photos differently, so the same look is not guaranteed. What cannot be converted is listed in the result and kept as it was.\n\nMissing originals are registered by their paths. Start the import?",
    ),
    ("안전 백업 만드는 중", "Making a safety backup"),
    ("Lightroom 기록 읽는 중", "Reading the Lightroom records"),
    ("사진 분류 가져오는 중", "Importing the photos and their labels"),
    ("컬렉션과 스택 정리 중", "Arranging collections and stacks"),
    ("진행 중인 사진 처리 작업이 끝난 뒤 가져오기를 시작하세요.", "Start the import after the running photo jobs finish."),
    ("가져오기를 취소했습니다. 이번에 추가한 기록은 모두 되돌렸습니다.", "The import was cancelled. Every record it added was rolled back."),
    ("추가 {0}장 · 이미 가져온 사진 {1}장 · 제외 {2}장", "Added {0} · already imported {1} · left out {2}"),
    ("추가한 컬렉션 {0}개 · 동적 스마트 컬렉션 {1}개", "Collections added {0} · dynamic smart collections {1}"),
    ("기존 Luma 사진의 보정과 분류는 유지했습니다.", "Edits and labels of the photos already in the library were kept."),
    ("사진 원본과 Lightroom 카탈로그는 수정하지 않았습니다.", "The original photos and the Lightroom catalog were not changed."),
    ("보정 변환 {0}장 · 이력 {1}개 · 스냅샷 {2}개", "Edits converted {0} · history steps {1} · snapshots {2}"),
    (
        "보정값은 Luma의 현상 방식으로 적용하므로 Adobe 결과와 색·명암·효과 강도가 다를 수 있습니다.",
        "The edits are applied with Grainy's own processing, so colours, tones and effect strengths can differ from Adobe's.",
    ),
    ("확인이 필요한 항목:", "Items to check:"),
    ("미변환 보정: {0}", "Edit not converted: {0}"),
    ("스마트 규칙 보관 이유: {0}", "Smart rules kept because: {0}"),
    ("• {0}: {1}개", "• {0}: {1}"),
    ("안전 백업: {0}", "Safety backup: {0}"),
    ("Adobe 현상값 보관 (Luma 보정으로 변환하지 않음)", "Adobe develop settings kept (not converted to edits)"),
    ("Adobe 보정 이력 보관 (실행 취소 이력으로 변환하지 않음)", "Adobe edit history kept (not converted to undo history)"),
    ("Adobe 스냅샷 보관 (Luma 스냅샷으로 변환하지 않음)", "Adobe snapshots kept (not converted to snapshots)"),
    ("컬렉션별 스택 정보 보관 (전역 스택으로 변환하지 않음)", "Per-collection stacks kept (not converted to stacks)"),
    ("키워드 동의어 보관 (검색 키워드로 변환하지 않음)", "Keyword synonyms kept (not converted to search keywords)"),
    ("스마트 컬렉션의 규칙 보관 (동적 검색은 적용하지 않음)", "Smart collection rules kept (no live search)"),
    ("순환 키워드 계층 표시 제외", "Keywords in a circular hierarchy left out"),
    ("구분 문자가 포함된 키워드 표시 제외", "Keywords containing a separator left out"),
    ("부모를 찾지 못한 키워드", "Keywords whose parent was not found"),
    ("경로 또는 파일 형식 미지원 사진 제외", "Photos with an unsupported path or format left out"),
    ("원본 연결을 확인할 수 없는 가상 사본 제외", "Virtual copies whose original cannot be confirmed left out"),
    ("순환 컬렉션 계층을 최상위에 배치", "Collections in a circular hierarchy placed at the top"),
    ("가져올 수 없는 컬렉션 사진 참조", "Collection entries of photos that could not be imported"),
    ("기존 Luma 사진이 포함된 스택 유지", "Stacks with photos already in the library kept as they were"),
    ("연결이 없는 스택 참조 제외", "Stack entries without a stack left out"),
    ("읽을 수 없는 보정 상태 (원문만 보관)", "Edit states that cannot be read (kept as they were)"),
    ("변환 가능한 보정 항목이 없는 상태 (원문만 보관)", "Edit states with nothing to convert (kept as they were)"),
    (
        "현재 보정 상태가 여러 개인 사진 (현재 보정은 원문만 보관)",
        "Photos with more than one current edit state (the current edits kept as they were)",
    ),
];

/// `tr` with this package's English for the texts the oracle's `en.json` lacks.
pub fn tr_lightroom(source: &str) -> String {
    let translated = tr(source);
    if translated != source || grainy_i18n::language() != grainy_i18n::Language::English {
        return translated;
    }
    ENGLISH.iter().find(|(k, _)| *k == source).map_or(translated, |(_, english)| (*english).to_owned())
}

fn tr_with(source: &str, values: &[&str]) -> String {
    let text = tr_lightroom(source);
    grainy_i18n::format(&text, values).unwrap_or(text)
}

/// Python's `f'{n:,}'`.
pub fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// `WARNING_LABELS`.
pub const WARNING_LABELS: [(&str, &str); 18] = [
    ("Adobe_imageDevelopSettings", "Adobe 현상값 보관 (Luma 보정으로 변환하지 않음)"),
    ("Adobe_libraryImageDevelopHistoryStep", "Adobe 보정 이력 보관 (실행 취소 이력으로 변환하지 않음)"),
    ("Adobe_libraryImageDevelopSnapshot", "Adobe 스냅샷 보관 (Luma 스냅샷으로 변환하지 않음)"),
    ("AgLibraryCollectionStackImage", "컬렉션별 스택 정보 보관 (전역 스택으로 변환하지 않음)"),
    ("AgLibraryKeywordSynonym", "키워드 동의어 보관 (검색 키워드로 변환하지 않음)"),
    ("smart_collection", "스마트 컬렉션의 규칙 보관 (동적 검색은 적용하지 않음)"),
    ("keyword_cycle", "순환 키워드 계층 표시 제외"),
    ("keyword_separator", "구분 문자가 포함된 키워드 표시 제외"),
    ("keyword_parent", "부모를 찾지 못한 키워드"),
    ("unsupported_path_or_format", "경로 또는 파일 형식 미지원 사진 제외"),
    ("virtual_master", "원본 연결을 확인할 수 없는 가상 사본 제외"),
    ("collection_cycle", "순환 컬렉션 계층을 최상위에 배치"),
    ("collection_member", "가져올 수 없는 컬렉션 사진 참조"),
    ("stack_existing", "기존 Luma 사진이 포함된 스택 유지"),
    ("stack_reference", "연결이 없는 스택 참조 제외"),
    ("develop_unreadable", "읽을 수 없는 보정 상태 (원문만 보관)"),
    ("develop_empty", "변환 가능한 보정 항목이 없는 상태 (원문만 보관)"),
    ("develop_ambiguous", "현재 보정 상태가 여러 개인 사진 (현재 보정은 원문만 보관)"),
];

fn int(result: &ImportResult, key: &str) -> i64 {
    result.get(key).and_then(Value::as_i64).unwrap_or(0)
}

/// Port of `lightroom_import.report_text(result)`.
pub fn report_text(result: &ImportResult) -> String {
    if result.get("cancelled").and_then(Value::as_bool).unwrap_or(false) {
        return tr_lightroom("가져오기를 취소했습니다. 이번에 추가한 기록은 모두 되돌렸습니다.");
    }
    let n = |key: &str| thousands(int(result, key));
    let mut lines = vec![
        tr_with("추가 {0}장 · 이미 가져온 사진 {1}장 · 제외 {2}장", &[&n("imported"), &n("reused"), &n("skipped")]),
        tr_with("추가한 컬렉션 {0}개 · 동적 스마트 컬렉션 {1}개", &[&n("collections"), &n("converted_smart")]),
        tr_lightroom("기존 Luma 사진의 보정과 분류는 유지했습니다."),
        tr_lightroom("사진 원본과 Lightroom 카탈로그는 수정하지 않았습니다."),
    ];
    if result.get("convert_edits").is_some_and(grainy_core::settings::truthy) {
        lines.push(tr_with(
            "보정 변환 {0}장 · 이력 {1}개 · 스냅샷 {2}개",
            &[&n("converted_photos"), &n("converted_history"), &n("converted_snapshots")],
        ));
        lines.push(tr_lightroom(
            "보정값은 Luma의 현상 방식으로 적용하므로 Adobe 결과와 색·명암·효과 강도가 다를 수 있습니다.",
        ));
    }
    if let Some(Value::Object(warnings)) = result.get("warnings").filter(|w| grainy_core::settings::truthy(w)) {
        lines.push(format!("\n{}", tr_lightroom("확인이 필요한 항목:")));
        for (key, count) in warnings {
            if let Some(version) = key.strip_prefix("adobe_process:") {
                lines.push(tr_with(
                    "Adobe 처리 버전 {0} → Grainy 현상 · {1}장",
                    &[version, &thousands(count.as_i64().unwrap_or(0))],
                ));
                continue;
            }
            let label = if let Some(field) = key.strip_prefix("develop_field:") {
                tr_with("미변환 보정: {0}", &[field])
            } else if let Some(reason) = key.strip_prefix("smart_reason:") {
                tr_with("스마트 규칙 보관 이유: {0}", &[reason])
            } else {
                WARNING_LABELS.iter().find(|(k, _)| k == key).map_or_else(|| key.clone(), |(_, l)| tr_lightroom(l))
            };
            lines.push(tr_with("• {0}: {1}개", &[&label, &thousands(count.as_i64().unwrap_or(0))]));
        }
    }
    let backup = result.get("backup").and_then(Value::as_str).unwrap_or("");
    lines.push(format!("\n{}", tr_with("안전 백업: {0}", &[backup])));
    lines.join("\n")
}

/// Port of the review box's text (`reviewed`): the count of photos and virtual copies and what is imported.
pub fn review_text(info: &Inspection) -> String {
    tr_with(
        "사진과 가상 사본 {0}개를 확인했습니다.\n\n별점·선택 표시·색 라벨·계층 키워드·IPTC·컬렉션과 폴더 스택을 가져옵니다.\n기존 Luma 보정과 분류를 덮어쓰지 않으며, 변경된 Lightroom 기록은 새 사본으로 가져옵니다.\n\n지원하는 스마트 컬렉션은 사진의 현재 분류·메타데이터로 검색합니다. 지원하지 않는 조건이 포함되면 전체 규칙을 보관하고 일반 컬렉션으로 가져옵니다.\n\n아래 선택을 켜면 지원하는 보정값·이력·스냅샷도 변환합니다. Luma와 Adobe의 현상 방식이 달라 같은 모습은 보장되지 않습니다. 변환하지 못한 항목은 결과에 표시하고 원문을 보관합니다.\n\n누락된 원본은 참조 경로로 등록됩니다. 가져오기를 진행할까요?",
        &[&thousands(info.count("Adobe_images"))],
    )
}

// ---- state --------------------------------------------------------------------------------------------------

/// The review box waiting for an answer.
#[derive(Debug, Clone)]
pub struct Review {
    /// The catalogue to import.
    pub path: PathBuf,
    /// What the inspection found.
    pub info: Inspection,
    /// The box's text.
    pub text: String,
    /// The 변환 check box (off at first).
    pub convert: bool,
}

/// What the Lightroom import keeps between frames.
#[derive(Default)]
pub struct LightroomState {
    /// The maintenance window's shared state while a run is open (`extras.lightroom_dialog`).
    pub shared: Option<Shared>,
    /// The review box's question.
    pub review: Option<Review>,
    /// `extras.last_lightroom_import`: the last import's result.
    pub last_import: Option<ImportResult>,
    /// The last failure shown (the oracle's traceback's last line), for tests.
    pub last_error: Option<String>,
    /// The clock of the imports (tests set a fixed one; the computer's otherwise).
    pub clock: Option<Arc<Mutex<dyn Clock>>>,
}

/// Gives the imports a clock (tests: [`import::TickingClock`]).
pub fn set_clock(cx: &mut AppContext, clock: Arc<Mutex<dyn Clock>>) {
    cx.state::<LightroomState>().clock = Some(clock);
}

/// Whether a Lightroom run (inspection, review or import) is open.
pub fn running(cx: &AppContext) -> bool {
    cx.state_ref::<LightroomState>().is_some_and(|s| s.shared.is_some())
}

// ---- the commands ---------------------------------------------------------------------------------------------

/// The `QMessageBox.warning` of `LibraryManager.run` when the command refuses.
fn refuse(cx: &mut AppContext, message: &str) {
    cx.state::<LightroomState>().last_error = Some(message.to_owned());
    cx.open_dialog(Box::new(Notice {
        id: "lightroom_refused",
        title: tr("작업을 완료하지 못했습니다"),
        text: tr_lightroom(message),
    }));
}

/// Port of `ExtraTools.import_lightroom`: refused while photo jobs run; the open dialog for a `.lrcat` (or the
/// path given as the invocation's value), then [`begin_import`].
fn import_lightroom(cx: &mut AppContext, invocation: &Invocation) {
    if cx.state::<FilesState>().maintenance || running(cx) {
        return;
    }
    if !cx.jobs.jobs_idle() || cx.exporting() {
        return refuse(cx, "진행 중인 사진 처리 작업이 끝난 뒤 가져오기를 시작하세요.");
    }
    if let Some(path) = invocation.value.as_ref().and_then(Value::as_str) {
        begin_import(cx, Path::new(path));
        return;
    }
    cx.open_dialog(Box::new(PathWait::open(
        tr("Lightroom 카탈로그 가져오기"),
        Pick::Open(Some(("Lightroom".to_owned(), vec!["lrcat"]))),
        None,
        |cx, path| begin_import(cx, &path),
    )));
}

/// Port of `begin_import(w, path)`: stores the open photo's change, opens the maintenance window and inspects
/// the catalogue on a job; the review box follows.
pub fn begin_import(cx: &mut AppContext, path: &Path) {
    let Some(directory) = cx.doc.library_folder() else { return };
    if crate::paths::is_oracle_folder(&directory, &crate::paths::Locations::current()) {
        return;
    }
    if let Err(message) = cx.doc.file_op(FileOp::Commit) {
        return show_error(cx, &message);
    }
    cx.state::<FilesState>().maintenance = true;
    let shared = Shared::default();
    let mut dialog = MaintenanceDialog::new(shared.clone());
    dialog.title = tr_lightroom("Lightroom 가져오기");
    dialog.note = tr_lightroom(
        "사진과 분류를 가져옵니다. 기존 Luma 사진은 별도 사본으로 보존합니다.\n지원하는 스마트 규칙을 연결하고, 보정값·이력 변환 여부를 다음 화면에서 선택합니다.",
    );
    dialog.label = tr("Lightroom 카탈로그 확인 중…");
    cx.open_dialog(Box::new(dialog));
    cx.state::<LightroomState>().shared = Some(shared.clone());
    let source = path.to_path_buf();
    let cancel = shared.cancel.clone();
    cx.jobs.spawn(
        "maintenance",
        tr("Lightroom 카탈로그 확인 중…"),
        move |_| import::inspect_catalog(&source, Some(&cancel)).map_err(|f| failure_text(&f)),
        {
            let path = path.to_path_buf();
            move |cx: &mut AppContext, result: Result<Inspection, String>| match result {
                Ok(info) => reviewed(cx, path, info),
                Err(message) => failed(cx, &message),
            }
        },
    );
}

fn failure_text(failure: &import::Failure) -> String {
    failure.text()
}

/// `release()`: the maintenance window closes and the run ends.
fn release(cx: &mut AppContext) {
    let state = cx.state::<LightroomState>();
    state.review = None;
    if let Some(shared) = state.shared.take() {
        shared.finish();
    }
    cx.state::<FilesState>().maintenance = false;
}

fn cancelled(cx: &mut AppContext) -> bool {
    cx.state::<LightroomState>().shared.as_ref().is_some_and(|s| s.cancel.is_set())
}

/// `failed(error)`: the run ends; the error box unless 취소 was pressed.
fn failed(cx: &mut AppContext, message: &str) {
    let was_cancelled = cancelled(cx);
    release(cx);
    if !was_cancelled {
        show_error(cx, message);
    }
}

/// `show_error(message)`: the status line and the 처리 오류 box with the message's last line.
fn show_error(cx: &mut AppContext, message: &str) {
    cx.state::<LightroomState>().last_error = Some(message.to_owned());
    cx.status = tr("처리하지 못했습니다. 자세한 내용을 확인해 주세요.");
    let last = message.trim().lines().last().unwrap_or("").to_owned();
    let mut message_box = MessageBox::warning("lightroom_error", tr("처리 오류"), last);
    message_box.detail = message.to_owned();
    cx.open_dialog(Box::new(message_box));
}

/// `reviewed(info)`: the review box opens over the maintenance window (nothing when 취소 was pressed meanwhile).
fn reviewed(cx: &mut AppContext, path: PathBuf, info: Inspection) {
    if cancelled(cx) {
        release(cx);
        return;
    }
    let text = review_text(&info);
    cx.state::<LightroomState>().review = Some(Review { path, info, text, convert: false });
    cx.open_dialog(Box::new(ReviewDialog));
}

/// The review box's answer: 가져오기 (`import`) with the 변환 check box (`convert`), or 취소. 가져오기 starts the
/// import job unless 취소 was pressed in the maintenance window meanwhile.
pub fn answer_review(cx: &mut AppContext, import: bool, convert: bool) {
    let Some(review) = cx.state::<LightroomState>().review.take() else { return };
    if !import || cancelled(cx) {
        release(cx);
        return;
    }
    let Some(shared) = cx.state::<LightroomState>().shared.clone() else { return };
    let Some(directory) = cx.doc.library_folder() else {
        release(cx);
        return;
    };
    shared.put(Update { stage: tr("안전 백업을 준비하고 있습니다…"), done: 0, total: 0 });
    let clock = cx.state::<LightroomState>().clock.clone();
    let progress = shared.clone();
    cx.jobs.spawn(
        "maintenance",
        tr_lightroom("Lightroom 가져오기"),
        move |_| {
            let mut report = |p: &import::Progress| {
                progress.put(Update { stage: tr_lightroom(p.stage), done: p.done, total: p.total });
                Ok(())
            };
            let run = |clock: &mut dyn Clock, report: import::Report<'_>| {
                import::import_catalog(&review.path, &directory, Some(&progress.cancel), report, convert, clock)
                    .map_err(|f| failure_text(&f))
            };
            match clock {
                Some(clock) => {
                    let mut guard = clock.lock().map_err(|_| "clock".to_owned())?;
                    run(&mut *guard, &mut report)
                }
                None => run(&mut SystemClock::default(), &mut report),
            }
        },
        move |cx: &mut AppContext, result: Result<ImportResult, String>| match result {
            Ok(result) => finished(cx, result),
            Err(message) => failed(cx, &message),
        },
    );
}

/// `finished(result)`: the run ends, the lists and collections are read again, and the result box opens.
fn finished(cx: &mut AppContext, result: ImportResult) {
    release(cx);
    let cancelled = result.get("cancelled").and_then(Value::as_bool).unwrap_or(false);
    let text = report_text(&result);
    cx.state::<LightroomState>().last_import = Some(result);
    if !cancelled {
        cx.doc.request_refresh(false);
        cx.state::<crate::session_state::ColumnState>().rescan();
    }
    cx.open_dialog(Box::new(MessageBox::information("lightroom_result", tr("Lightroom 가져오기 결과"), text)));
}

/// The stored summary of the latest import (`lightroom_import_runs`, newest first), as the result dict.
pub fn latest_summary(catalog: &grainy_catalog::Catalog) -> Option<ImportResult> {
    let db = catalog.connection();
    let exists: Option<i64> =
        db.query_row("SELECT 1 FROM sqlite_master WHERE name='lightroom_import_runs'", [], |r| r.get(0)).ok();
    exists?;
    let text: String = db
        .query_row("SELECT summary FROM lightroom_import_runs ORDER BY created DESC LIMIT 1", [], |r| r.get(0))
        .ok()?;
    match serde_json::from_str(&text) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// Port of `ExtraTools.lightroom_report`: the latest stored result, or 저장된 가져오기 결과가 없습니다.
fn lightroom_report(cx: &mut AppContext, _: &Invocation) {
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    let message = match latest_summary(catalog) {
        Some(summary) => {
            MessageBox::information("lightroom_report", tr("최근 Lightroom 가져오기 결과"), report_text(&summary))
        }
        None => MessageBox::information(
            "lightroom_report",
            tr("Lightroom 가져오기"),
            tr("저장된 가져오기 결과가 없습니다."),
        ),
    };
    cx.open_dialog(Box::new(message));
}
