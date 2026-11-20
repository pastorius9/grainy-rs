//! The 파일 관리 menu's file and catalogue commands (work package 22): luma/manager.py `rename`, `move`,
//! `move_selected_folder`, `remove(True)` (원본을 휴지통으로), `relink`, `duplicates`, `smart_previews`, `backup`,
//! `restore`, `integrity`, `optimize_catalog`, `relink_folder`; luma/preview_queue.py `start_batch` / `stop`
//! (선택 사진 미리보기 다시 만들기, 일괄 미리보기 중단). The folder tree's 폴더 위치 다시 지정… and 이름이 바뀐 폴더 자동 찾기
//! are in [`crate::folders`].
//!
//! Each command opens the oracle's box (a name box, a question, a native file dialog) and then calls the public
//! function that does the work (`rename_with`, `move_to`, …); tests call those directly. A failure shows the box of
//! `LibraryManager.run` (작업을 완료하지 못했습니다).
//!
//! **Data safety.** Moves and renames go through grainy-library's `move_files` / `move_folder` (an existing target
//! is refused, completed moves are undone on failure). 원본을 휴지통으로 is the only command that removes a file from
//! its folder, and it does so through [`Trash`]: the system's recycle bin in the app, a fake in tests. Backups,
//! restores, optimisation and checks work on the document's own library only (`Document::library_folder`, which
//! is `None` for the read-only Python Grainy library).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use grainy_catalog::folders::Platform;
use grainy_library::file_access::{access, Access};
use grainy_library::pathx;
use grainy_library::pyformat::{self, Arg};
use serde_json::{json, Value};

use super::{Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::about::MessageBox;
use crate::dialogs::confirm::Confirm;
use crate::dialogs::duplicates::DuplicatesDialog;
use crate::dialogs::filter::TextBox;
use crate::dialogs::import_errors::Notice;
use crate::dialogs::maintenance::{MaintenanceDialog, PathWait, Pick, Shared, Update};
use crate::document::catalog::files::{paths_of, FileOp, FileReply};
use crate::document::ThumbCache;
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "file.rename", label: "선택 사진 이름 변경", enabled: has_selection, run: rename },
    Command { id: "file.move", label: "선택 사진 이동", enabled: has_selection, run: move_photos },
    Command { id: "file.move_folder", label: "폴더 이름 변경·이동", enabled: writable, run: move_folder },
    Command { id: "file.trash", label: "원본을 휴지통으로", enabled: has_selection, run: trash },
    Command { id: "file.relink", label: "누락 원본 다시 연결", enabled: photo_writable, run: relink },
    Command { id: "file.duplicates", label: "중복 파일 검사", enabled: has_catalog, run: duplicates },
    Command {
        id: "file.smart_previews",
        label: "오프라인 편집용 미리보기 만들기",
        enabled: has_selection,
        run: smart_previews,
    },
    Command { id: "file.backup", label: "카탈로그 백업", enabled: writable, run: backup },
    Command { id: "file.restore", label: "카탈로그 복원", enabled: writable, run: restore },
    Command { id: "file.optimize", label: "카탈로그 최적화", enabled: writable, run: optimize },
    Command { id: "file.integrity", label: "카탈로그 검사", enabled: has_catalog, run: integrity },
    Command {
        id: "file.rebuild_previews",
        label: "선택 사진 미리보기 다시 만들기",
        enabled: has_selection,
        run: rebuild_previews,
    },
    Command {
        id: "file.stop_previews", label: "일괄 미리보기 중단", enabled: batch_running, run: stop_previews
    },
    Command { id: "file.relink_folder", label: "누락 폴더 다시 연결", enabled: writable, run: relink_folder },
    Command {
        id: "library.locate_folder", label: "폴더 위치 다시 지정…", enabled: writable, run: locate_folder
    },
    Command {
        id: "library.recover_folders",
        label: "이름이 바뀐 폴더 자동 찾기",
        enabled: writable,
        run: crate::folders::recover_command,
    },
];

// ---- texts --------------------------------------------------------------------------------------------------

/// English for the oracle's texts its `en.json` lacks (shown untranslated by the Python Grainy).
const ENGLISH: &[(&str, &str)] = &[
    (
        "원본 파일은 휴지통으로 이동하며 같은 원본의 가상 사본도 제거됩니다.",
        "The original files go to the Recycle Bin, and virtual copies of the same originals are removed too.",
    ),
    (
        "프로파일 중복과 빈 공간을 정리합니다.\n원본 사진은 그대로 유지합니다. 취소 후에도 다시 시작할 수 있습니다.",
        "Duplicate profiles and empty space are cleaned up.\nOriginal photos stay as they are. You can start again after cancelling.",
    ),
    ("진행 중인 사진 처리 작업이 끝난 뒤 최적화를 시작하세요.", "Start the optimization after the running photo jobs finish."),
    ("왼쪽에서 누락된 폴더를 선택하세요.", "Choose the missing folder on the left."),
    ("왼쪽에서 이동할 사진 폴더를 선택하세요.", "Choose the photo folder to move on the left."),
    ("파일 이름에 경로나 특수문자를 넣을 수 없습니다.", "File names cannot contain paths or special characters."),
    ("가져오기·내보내기 완료 후 복원하세요.", "Restore after the import or export has finished."),
    ("그룹 {0} · {1}개", "Group {0} · {1} files"),
    ("갱신 완료", "Updated"),
    ("최신", "Up to date"),
    ("제거됨", "Removed"),
    ("변경됨 · 다시 확인", "Changed · check again"),
    ("정리한 기록: {0}개 · 보관한 프로파일: {1}개", "Records cleaned: {0} · profiles kept: {1}"),
    ("카탈로그: {0} → {1} MiB (백업 제외)", "Catalog: {0} → {1} MiB (backups excluded)"),
    ("무결성: {0}", "Integrity: {0}"),
    ("자동 백업: {0}", "Automatic backup: {0}"),
    ("확인이 필요한 기록:", "Records to check:"),
];

/// `tr` with this package's English table.
pub fn tr_files(source: &str) -> String {
    if grainy_i18n::language() == grainy_i18n::Language::English {
        if let Some((_, english)) = ENGLISH.iter().find(|(k, _)| *k == source) {
            return (*english).to_owned();
        }
    }
    tr(source)
}

/// [`tr_files`] with `{0}`, `{1}`… filled.
pub fn tr_files_with(source: &str, values: &[&str]) -> String {
    let text = tr_files(source);
    grainy_i18n::format(&text, values).unwrap_or(text)
}

// ---- state --------------------------------------------------------------------------------------------------

/// The recycle bin (`send2trash`). The app uses the system's; tests give a fake ([`set_trash`]).
pub trait Trash {
    /// Moves `path` to the recycle bin.
    fn trash(&self, path: &Path) -> Result<(), String>;
}

/// The system's recycle bin (the `trash` crate, through `grainy_platform::updater::recycle`).
pub struct SystemTrash;

impl Trash for SystemTrash {
    fn trash(&self, path: &Path) -> Result<(), String> {
        if grainy_platform::updater::recycle(path) {
            Ok(())
        } else {
            Err(format!("휴지통으로 옮기지 못했습니다: {}", path.display()))
        }
    }
}

/// `PreviewQueue`'s batch (선택 사진 미리보기 다시 만들기).
#[derive(Default)]
pub struct PreviewBatch {
    pub queue: VecDeque<u64>,
    pub total: usize,
    pub completed: usize,
    pub failures: usize,
    pub running: bool,
    pub cancel: Arc<AtomicBool>,
}

/// What file management keeps between frames.
#[derive(Default)]
pub struct FilesState {
    trash: Option<Arc<dyn Trash>>,
    pub batch: PreviewBatch,
    /// `maintenance_running`.
    pub maintenance: bool,
    /// The last optimisation's report (`last_optimization`).
    pub last_optimization: Option<grainy_library::catalog_maintenance::Report>,
    /// The message of the last failure box (as the oracle's `str(e)`), for tests.
    pub last_error: Option<String>,
}

/// Replaces the recycle bin (tests: a fake that never touches the system's).
pub fn set_trash(cx: &mut AppContext, trash: Arc<dyn Trash>) {
    cx.state::<FilesState>().trash = Some(trash);
}

// ---- enabled ------------------------------------------------------------------------------------------------

fn writable(cx: &AppContext) -> bool {
    cx.doc.library_folder().is_some()
}

fn has_catalog(cx: &AppContext) -> bool {
    cx.doc.metadata_catalog().is_some()
}

fn has_selection(cx: &AppContext) -> bool {
    writable(cx) && !selected_ids(cx).is_empty()
}

fn photo_writable(cx: &AppContext) -> bool {
    writable(cx) && cx.doc.current().is_some()
}

fn batch_running(cx: &AppContext) -> bool {
    cx.state_ref::<FilesState>().is_some_and(|s| s.batch.total > 0)
}

/// `selected_ids()`: the selected photos, or the open one when none is selected.
pub fn selected_ids(cx: &AppContext) -> Vec<u64> {
    let ids = crate::commands::collections::selected_ids(cx);
    if ids.is_empty() {
        cx.doc.current().map(|p| p.id).into_iter().collect()
    } else {
        ids
    }
}

// ---- shared helpers -----------------------------------------------------------------------------------------

/// The box `LibraryManager.run` shows when an action raises.
pub fn fail(cx: &mut AppContext, message: &str) {
    cx.state::<FilesState>().last_error = Some(message.to_owned());
    cx.open_dialog(Box::new(Notice {
        id: "files_error",
        title: tr("작업을 완료하지 못했습니다"),
        text: tr_files(message),
    }));
}

/// Runs a file operation; the open photo is refreshed when it changed. `Err` shows the failure box.
fn op(cx: &mut AppContext, op: FileOp) -> Option<FileReply> {
    let had = cx.doc.current_id();
    let result = cx.doc.file_op(op);
    if cx.doc.current_id() != had {
        cx.photo_changed();
    }
    match result {
        Ok(reply) => Some(reply),
        Err(message) => {
            fail(cx, &message);
            None
        }
    }
}

fn count(reply: Option<FileReply>) -> Option<usize> {
    match reply? {
        FileReply::Count(n) => Some(n),
        FileReply::Done => Some(0),
    }
}

fn value_str(invocation: &Invocation) -> Option<String> {
    invocation.value.as_ref().and_then(Value::as_str).map(str::to_owned)
}

/// `Catalog.photo(id)` rows of the selection.
fn selected_rows(cx: &AppContext) -> Vec<Value> {
    let Some(catalog) = cx.doc.metadata_catalog() else { return Vec::new() };
    selected_ids(cx).iter().filter_map(|id| catalog.photo(*id as i64).ok().flatten()).collect()
}

fn platform() -> Platform {
    Platform::native()
}

// ---- rename / move ------------------------------------------------------------------------------------------

/// 선택 사진 이름 변경: the template box (`{stem}_{n:03d}`), then [`rename_with`].
fn rename(cx: &mut AppContext, invocation: &Invocation) {
    if let Some(template) = value_str(invocation) {
        rename_with(cx, &template);
        return;
    }
    cx.open_dialog(Box::new(TextBox::new(
        "files_rename",
        tr("파일 이름 변경"),
        tr("{stem}: 원래 이름 / {n:03d}: 연속 번호"),
        "{stem}_{n:03d}",
        |cx: &mut AppContext, template: String| rename_with(cx, &template),
    )));
}

/// Port of `manager.rename`'s mapping: the selected photos' files (each once) to `template.format(stem=…, n=…)`
/// plus the old suffix, next to the old file. Names with a folder part or one of `<>:"/\|?*` are refused.
pub fn rename_mapping(paths: &[String], template: &str) -> Result<Vec<(String, String)>, String> {
    let p = platform();
    let mut mapping = Vec::new();
    for (n, source) in paths.iter().enumerate() {
        let (stem, suffix) = pathx::stem_suffix(&pathx::name(p, source));
        let formatted = pyformat::format(template, &[("stem", Arg::Str(stem)), ("n", Arg::Int(n as i64 + 1))])
            .map_err(|e| {
                // `str(KeyError('name'))` is `'name'`.
                let text = e.to_string();
                text.strip_prefix("KeyError: ").map_or(text.clone(), str::to_owned)
            })?;
        let name = formatted + &suffix;
        if pathx::name(p, &name) != name || name.chars().any(|c| "<>:\"/\\|?*".contains(c)) {
            return Err("파일 이름에 경로나 특수문자를 넣을 수 없습니다.".into());
        }
        mapping.push((source.clone(), pathx::with_name(p, source, &name)));
    }
    Ok(mapping)
}

/// What 확인 of the rename box does (`manager.rename` after `getText`).
pub fn rename_with(cx: &mut AppContext, template: &str) {
    if template.is_empty() {
        return;
    }
    let ids = selected_ids(cx);
    let paths = match cx.doc.metadata_catalog().map(|c| paths_of(c, &ids)) {
        Some(Ok(paths)) => paths,
        Some(Err(message)) => return fail(cx, &message),
        None => return,
    };
    let mapping = match rename_mapping(&paths, template) {
        Ok(mapping) => mapping,
        Err(message) => return fail(cx, &message),
    };
    if let Some(n) = count(op(cx, FileOp::MoveFiles { mapping })) {
        cx.status = tr_with("{0}개 파일 이름을 변경했습니다.", &[&n.to_string()]);
    }
}

/// 선택 사진 이동: the folder dialog, then [`move_to`].
fn move_photos(cx: &mut AppContext, invocation: &Invocation) {
    if let Some(folder) = value_str(invocation) {
        move_to(cx, Path::new(&folder));
        return;
    }
    cx.open_dialog(Box::new(PathWait::open(
        tr("선택 사진을 이동할 폴더"),
        Pick::Folder,
        None,
        |cx, folder| {
            move_to(cx, &folder);
        },
    )));
}

/// Port of `manager.move` after the folder dialog: every selected photo's file into `folder` under its name.
pub fn move_to(cx: &mut AppContext, folder: &Path) {
    let p = platform();
    let folder = pathx::text(folder);
    let mut mapping: Vec<(String, String)> = Vec::new();
    for row in selected_rows(cx) {
        let source = row["path"].as_str().unwrap_or("").to_owned();
        let target = pathx::join(p, &folder, &pathx::name(p, &source));
        match mapping.iter_mut().find(|(s, _)| *s == source) {
            Some(entry) => entry.1 = target,
            None => mapping.push((source, target)),
        }
    }
    op(cx, FileOp::MoveFiles { mapping });
}

/// 폴더 이름 변경·이동: the folder chosen in the tree, the box with its full path, then [`move_folder_to`].
fn move_folder(cx: &mut AppContext, invocation: &Invocation) {
    let Some(source) = cx.doc.query().folder else {
        return fail(cx, "왼쪽에서 이동할 사진 폴더를 선택하세요.");
    };
    if let Some(destination) = value_str(invocation) {
        move_folder_to(cx, &source, &destination);
        return;
    }
    let initial = source.clone();
    cx.open_dialog(Box::new(TextBox::new(
        "files_move_folder",
        tr("폴더 이름 변경·이동"),
        tr("새 폴더의 전체 경로"),
        &initial,
        move |cx: &mut AppContext, destination: String| move_folder_to(cx, &source, &destination),
    )));
}

/// Port of `manager.move_selected_folder` after its box: the folder moves on disk with every path below it, and
/// the tree's choice follows it (`folder_filter = str(Path(destination).resolve())`).
pub fn move_folder_to(cx: &mut AppContext, source: &str, destination: &str) {
    let p = platform();
    if destination.is_empty() || p.path_key(destination) == p.path_key(source) {
        return;
    }
    let reply = op(cx, FileOp::MoveFolder { source: source.to_owned(), destination: destination.to_owned() });
    if reply.is_some() {
        let resolved = pathx::resolve(p, destination);
        cx.change_query(|q| q.folder = Some(resolved));
        cx.state::<crate::session_state::ColumnState>().rescan();
    }
}

// ---- remove to the recycle bin ------------------------------------------------------------------------------

/// 원본을 휴지통으로: the question of `manager.remove(True)`, then [`trash_now`].
fn trash(cx: &mut AppContext, _: &Invocation) {
    let ids = selected_ids(cx);
    if ids.is_empty() {
        return;
    }
    let text = format!(
        "{}\n{}",
        tr_with("선택한 {0}장을 카탈로그에서 제거합니다.", &[&ids.len().to_string()]),
        tr_files("원본 파일은 휴지통으로 이동하며 같은 원본의 가상 사본도 제거됩니다.")
    );
    cx.open_dialog(Box::new(Confirm::new("files_trash", tr("사진 제거"), text, move |cx: &mut AppContext| {
        trash_now(cx, &ids);
    })));
}

/// Port of `manager.remove(True)` after 예: the pending edit is stored, each photo's file (when it is there) goes
/// to the recycle bin, and every row with one of those paths (virtual copies too) leaves the catalogue. A file the
/// bin refuses stops the removal before the catalogue changes (the oracle's `send2trash` raising).
pub fn trash_now(cx: &mut AppContext, ids: &[u64]) {
    if op(cx, FileOp::Commit).is_none() {
        return;
    }
    let paths = match cx.doc.metadata_catalog().map(|c| paths_of(c, ids)) {
        Some(Ok(paths)) => paths,
        Some(Err(message)) => return fail(cx, &message),
        None => return,
    };
    for path in &paths {
        if let Err(error) = crate::paths::writable(Path::new(path)) {
            return fail(cx, &error.to_string());
        }
    }
    let bin: Arc<dyn Trash> = cx.state::<FilesState>().trash.clone().unwrap_or_else(|| Arc::new(SystemTrash));
    for path in &paths {
        if Path::new(path).exists() {
            if let Err(message) = bin.trash(Path::new(path)) {
                return fail(cx, &message);
            }
        }
    }
    op(cx, FileOp::RemovePaths { paths });
}

// ---- relink -------------------------------------------------------------------------------------------------

/// 누락 원본 다시 연결: the file dialog, then [`relink_to`].
fn relink(cx: &mut AppContext, invocation: &Invocation) {
    if cx.doc.current().is_none() {
        return;
    }
    if let Some(path) = value_str(invocation) {
        relink_to(cx, Path::new(&path));
        return;
    }
    cx.open_dialog(Box::new(PathWait::open(tr("원본 사진 다시 연결"), Pick::Open(None), None, |cx, path| {
        relink_to(cx, &path);
    })));
}

/// Port of `manager.relink` after the file dialog: the open photo (and every row with its path) points at `path`.
pub fn relink_to(cx: &mut AppContext, path: &Path) {
    let Some(id) = cx.doc.current().map(|p| p.id) else { return };
    op(cx, FileOp::Relink { id, path: pathx::text(path) });
}

/// 누락 폴더 다시 연결 (`manager.relink_folder`): the folder chosen in the tree is located again.
fn relink_folder(cx: &mut AppContext, _: &Invocation) {
    let Some(folder) = cx.doc.query().folder else {
        return fail(cx, "왼쪽에서 누락된 폴더를 선택하세요.");
    };
    crate::folders::locate(cx, &folder);
}

/// 폴더 위치 다시 지정… of the folder tree (the folder's path is the invocation's value).
fn locate_folder(cx: &mut AppContext, invocation: &Invocation) {
    if let Some(folder) = value_str(invocation) {
        crate::folders::locate(cx, &folder);
    }
}

// ---- duplicates ---------------------------------------------------------------------------------------------

/// Port of `manager.duplicates`: the catalogue's files are compared by size, then by SHA-256 of their contents, on
/// a job; the groups open in a window, or a box says there are none.
fn duplicates(cx: &mut AppContext, _: &Invocation) {
    let Some(photos) = cx.doc.metadata_catalog().and_then(|c| c.photos().ok()) else { return };
    cx.status = tr("파일 내용으로 중복 검사 중…");
    cx.jobs.spawn(
        "files",
        tr("중복 파일 검사"),
        move |_| grainy_library::library::duplicate_groups(platform(), &photos).map_err(|e| e.to_string()),
        |cx: &mut AppContext, result| match result {
            Ok(groups) => show_duplicates(cx, groups),
            Err(message) => show_error(cx, &message),
        },
    );
}

/// What the duplicate search's `done` shows.
pub fn show_duplicates(cx: &mut AppContext, groups: Vec<Vec<Value>>) {
    if groups.is_empty() {
        cx.open_dialog(Box::new(MessageBox::information(
            "files_duplicates_none",
            tr("중복 검사"),
            tr("내용이 같은 파일이 없습니다."),
        )));
    } else {
        cx.close_dialog("duplicates");
        cx.open_dialog(Box::new(DuplicatesDialog::new(groups)));
    }
}

/// `show_error(message)`: the status line and the 처리 오류 box with the last line of the message.
fn show_error(cx: &mut AppContext, message: &str) {
    cx.status = tr("처리하지 못했습니다. 자세한 내용을 확인해 주세요.");
    let last = message.trim().lines().last().unwrap_or("").to_owned();
    let mut message_box = MessageBox::warning("files_error_box", tr("처리 오류"), last);
    message_box.detail = message.to_owned();
    cx.open_dialog(Box::new(message_box));
}

// ---- offline packets ----------------------------------------------------------------------------------------

/// Port of `manager.smart_previews`: for each selected photo a packet `previews/<id>.npz` of the library (the
/// photo loaded at 1800 pixels with its settings: float16 pixels, or float32 camera pixels with the old-style
/// develop as `legacy_pixels` for a RAW file decoded natively), so it can be edited while the original is offline.
fn smart_previews(cx: &mut AppContext, _: &Invocation) {
    let photos = selected_rows(cx);
    let Some(folder) = cx.doc.library_folder() else { return };
    if photos.is_empty() {
        return;
    }
    let directory = folder.join("previews");
    if let Err(error) = std::fs::create_dir_all(&directory) {
        return fail(cx, &error.to_string());
    }
    cx.status = tr("오프라인 편집 미리보기 만드는 중…");
    cx.jobs.spawn(
        "files",
        tr("오프라인 편집용 미리보기 만들기"),
        move |handle| {
            for (i, photo) in photos.iter().enumerate() {
                if handle.cancelled() {
                    break;
                }
                handle.progress(i, photos.len(), "");
                write_packet(&directory, photo)?;
            }
            Ok(photos.len())
        },
        |cx: &mut AppContext, result| match result {
            Ok(count) => {
                cx.status = tr_with(
                    "{0}장: 원본 없이도 편집할 수 있습니다. 원본 출력에는 원본 연결이 필요합니다.",
                    &[&count.to_string()],
                );
            }
            Err(message) => show_error(cx, &message),
        },
    );
}

/// Writes one offline packet (the body of `smart_previews`' loop).
pub fn write_packet(directory: &Path, photo: &Value) -> Result<(), String> {
    use grainy_catalog::preview::{write_offline, OfflinePacket, Pixels};
    use grainy_engine::files::{load_photo, Photo};
    let path = photo["path"].as_str().unwrap_or("");
    let settings = match &photo["settings"] {
        Value::Object(map) => grainy_core::normalized(map),
        _ => grainy_core::normalized(&serde_json::Map::new()),
    };
    let loaded = load_photo(path, Some(1800), &settings).map_err(|e| e.to_string())?;
    let mut info = loaded.metadata;
    let camera = matches!(loaded.photo, Photo::Camera(_));
    let pixels_of = |photo: &Photo, half: bool| {
        let pixels = photo.pixels();
        let (h, w, c) = pixels.dim();
        Pixels { shape: [h, w, c], values: pixels.iter().copied().collect(), half }
    };
    let mut legacy = None;
    if camera {
        // `load_image(path, 1800, working_space)` without the RAW options: the old-style develop.
        let mut plain = settings.clone();
        plain.0.insert("raw_mode".into(), json!("legacy"));
        plain.0.insert("dcp_profile".into(), json!(""));
        plain.0.insert("negative".into(), json!(false));
        let old = load_photo(path, Some(1800), &plain).map_err(|e| e.to_string())?;
        legacy = Some(pixels_of(&old.photo, false));
        info.insert("raw_dual_preview".into(), json!(true));
    }
    let packet =
        OfflinePacket { pixels: pixels_of(&loaded.photo, !camera), info: Value::Object(info), legacy_pixels: legacy };
    let id = photo["id"].as_i64().unwrap_or(0);
    write_offline(directory.join(format!("{id}.npz")), &packet).map_err(|e| e.to_string())
}

// ---- backup and restore -------------------------------------------------------------------------------------

fn sqlite_filter() -> (String, Vec<&'static str>) {
    ("SQLite".to_owned(), vec!["sqlite"])
}

/// 카탈로그 백업: the save dialog (`Grainy-<time>.sqlite`), then [`backup_to`].
fn backup(cx: &mut AppContext, invocation: &Invocation) {
    if let Some(path) = value_str(invocation) {
        backup_to(cx, Path::new(&path));
        return;
    }
    let name = format!("Grainy-{}.sqlite", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    cx.open_dialog(Box::new(PathWait::open(
        tr("카탈로그 백업"),
        Pick::Save(name, sqlite_filter()),
        None,
        |cx, path| {
            backup_to(cx, &path);
        },
    )));
}

/// Port of `manager.backup` after the save dialog.
pub fn backup_to(cx: &mut AppContext, path: &Path) {
    if op(cx, FileOp::Backup { path: path.to_path_buf() }).is_some() {
        cx.open_dialog(Box::new(MessageBox::information(
            "files_backup",
            tr("백업"),
            tr("카탈로그·보정·컬렉션을 백업했습니다. 원본 사진은 별도로 보관하세요."),
        )));
    }
}

fn transfers_running(cx: &AppContext) -> bool {
    !cx.jobs.jobs_idle() || !cx.renderer.idle() || crate::ai::running_job(cx).is_some()
}

/// 카탈로그 복원: refused while an import or export runs; the open dialog, the question, then [`restore_from`].
fn restore(cx: &mut AppContext, invocation: &Invocation) {
    if transfers_running(cx) {
        return fail(cx, "가져오기·내보내기 완료 후 복원하세요.");
    }
    let ask = |cx: &mut AppContext, path: PathBuf| {
        cx.open_dialog(Box::new(Confirm::new(
            "files_restore",
            tr("카탈로그 복원"),
            tr("현재 카탈로그를 자동 백업한 뒤 선택한 백업으로 복원할까요?"),
            move |cx: &mut AppContext| restore_from(cx, &path),
        )));
    };
    if let Some(path) = value_str(invocation) {
        ask(cx, PathBuf::from(path));
        return;
    }
    cx.open_dialog(Box::new(PathWait::open(tr("카탈로그 백업 복원"), Pick::Open(Some(sqlite_filter())), None, ask)));
}

/// Port of `manager.restore` after 예: the preview batch stops, the catalogue is replaced by the backup (a copy of
/// the current one is kept first), and the list is read again without collection, rules or folder.
pub fn restore_from(cx: &mut AppContext, path: &Path) {
    if transfers_running(cx) {
        return fail(cx, "가져오기·내보내기 완료 후 복원하세요.");
    }
    stop_batch(cx);
    if op(cx, FileOp::Restore { path: path.to_path_buf() }).is_some() {
        cx.photo_changed();
        cx.state::<crate::session_state::ColumnState>().rescan();
    }
}

// ---- integrity and optimisation -----------------------------------------------------------------------------

/// The text of the 카탈로그 검사 box.
pub fn integrity_text(status: &str, photos: usize, missing: usize, denied: usize) -> String {
    let mut text = tr_with(
        "데이터베이스: {0}\n사진 {1}장 / 누락된 원본 참조 {2}개",
        &[status, &photos.to_string(), &missing.to_string()],
    );
    if denied > 0 {
        text += &tr_with("\n접근 권한이 필요한 원본 {0}개 (누락 아님)", &[&denied.to_string()]);
    }
    text
}

/// Port of `manager.integrity`: `PRAGMA integrity_check`, then the originals that are missing or refused, counted
/// on a job.
fn integrity(cx: &mut AppContext, _: &Invocation) {
    let Some(catalog) = cx.doc.metadata_catalog() else { return };
    // For a read-only library this is the private snapshot directory.
    let directory = catalog.directory().to_owned();
    cx.spawn_document_job(
        "files",
        tr("카탈로그 검사"),
        move |handle| {
            let db = grainy_catalog::Catalog::open_reader(&directory).map_err(|e| e.to_string())?;
            let status: String =
                db.connection().query_row("PRAGMA integrity_check", [], |r| r.get(0)).map_err(|e| e.to_string())?;
            let mut statement = db.connection().prepare("SELECT path FROM photos").map_err(|e| e.to_string())?;
            let paths = statement.query_map([], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?;
            let (mut count, mut missing, mut denied) = (0, 0, 0);
            for path in paths {
                if handle.cancelled() {
                    return Err("cancelled".into());
                }
                count += 1;
                match access(Path::new(&path.map_err(|e| e.to_string())?), false) {
                    Access::Missing => missing += 1,
                    Access::Denied => denied += 1,
                    _ => {}
                }
            }
            Ok((status, count, missing, denied))
        },
        move |cx, result| match result {
            Ok((status, total, missing, denied)) => {
                cx.open_dialog(Box::new(MessageBox::information(
                    "files_integrity",
                    tr("카탈로그 검사"),
                    integrity_text(&status, total, missing, denied),
                )));
            }
            Err(message) => show_error(cx, &message),
        },
    );
}

/// The text of the box at the end of an optimisation (`optimize_catalog`'s `finished`).
pub fn optimize_text(report: &grainy_library::catalog_maintenance::Report) -> String {
    let mib = |bytes: u64| format!("{:.2}", bytes as f64 / 1024f64.powi(2));
    let mut text = if report.cancelled {
        tr("최적화를 취소했습니다. 완료된 정리는 유지됩니다.")
    } else {
        tr("카탈로그 최적화를 마쳤습니다.")
    };
    text += "\n";
    text += &tr_files_with(
        "정리한 기록: {0}개 · 보관한 프로파일: {1}개",
        &[&report.converted.to_string(), &report.profiles.to_string()],
    );
    text += "\n";
    text +=
        &tr_files_with("카탈로그: {0} → {1} MiB (백업 제외)", &[&mib(report.before_bytes), &mib(report.after_bytes)]);
    text += "\n";
    text += &tr_files_with("무결성: {0}", &[&report.integrity]);
    if let Some(backup) = &report.backup {
        text += "\n";
        text += &tr_files_with("자동 백업: {0}", &[backup]);
    }
    if !report.warnings.is_empty() {
        text += "\n";
        text += &tr_files("확인이 필요한 기록:");
        text += "\n";
        text += &report.warnings.join("\n");
    }
    text
}

/// Port of `manager.optimize_catalog`: refused while photo jobs run; the open photo's change is stored, the
/// maintenance window opens, and `catalog_maintenance.optimize` runs on the library's folder on a job.
fn optimize(cx: &mut AppContext, _: &Invocation) {
    if cx.state::<FilesState>().maintenance {
        return;
    }
    if transfers_running(cx) || cx.jobs.running("files") || cx.jobs.running("previews") {
        return fail(cx, "진행 중인 사진 처리 작업이 끝난 뒤 최적화를 시작하세요.");
    }
    let Some(folder) = cx.doc.library_folder() else { return };
    if crate::paths::is_oracle_folder(&folder, &crate::paths::Locations::current()) {
        return;
    }
    if op(cx, FileOp::Commit).is_none() {
        return;
    }
    let shared = Shared::default();
    cx.state::<FilesState>().maintenance = true;
    cx.open_dialog(Box::new(MaintenanceDialog::new(shared.clone())));
    let progress = shared.clone();
    cx.jobs.spawn(
        "maintenance",
        tr("카탈로그 최적화"),
        move |_| {
            let mut report = |p: grainy_library::catalog_maintenance::Progress| {
                progress.put(Update { stage: p.stage, done: p.examined, total: p.total });
            };
            grainy_library::catalog_maintenance::optimize(&folder, Some(&progress.cancel), &mut report)
                .map_err(|e| e.to_string())
        },
        move |cx: &mut AppContext, result| {
            shared.finish();
            cx.state::<FilesState>().maintenance = false;
            match result {
                Ok(report) => {
                    let text = optimize_text(&report);
                    cx.state::<FilesState>().last_optimization = Some(report);
                    cx.open_dialog(Box::new(MessageBox::information("files_optimize", tr("카탈로그 최적화"), text)));
                }
                Err(message) => show_error(cx, &message),
            }
        },
    );
}

// ---- the preview batch --------------------------------------------------------------------------------------

/// 선택 사진 미리보기 다시 만들기 (`preview_queue.start_batch(selected_ids())`).
fn rebuild_previews(cx: &mut AppContext, _: &Invocation) {
    let ids = selected_ids(cx);
    start_batch(cx, &ids);
}

/// Port of `PreviewQueue.start_batch`: the photos' stored thumbnails are made again, one at a time.
pub fn start_batch(cx: &mut AppContext, ids: &[u64]) {
    stop_batch(cx);
    let batch = &mut cx.state::<FilesState>().batch;
    let mut seen = Vec::new();
    for id in ids {
        if !seen.contains(id) {
            seen.push(*id);
        }
    }
    batch.total = seen.len();
    batch.queue = seen.into();
    batch.completed = 0;
    batch.failures = 0;
    next(cx);
}

/// 일괄 미리보기 중단 (`PreviewQueue.stop`).
fn stop_previews(cx: &mut AppContext, _: &Invocation) {
    stop_batch(cx);
}

/// Port of `PreviewQueue.stop`.
pub fn stop_batch(cx: &mut AppContext) {
    let batch = &mut cx.state::<FilesState>().batch;
    batch.cancel.store(true, Ordering::SeqCst);
    batch.cancel = Arc::new(AtomicBool::new(false));
    batch.queue.clear();
    batch.total = 0;
}

/// Whether a status counts as a failure (`status.startswith(('오류','원본 없음','접근 권한 필요'))`).
pub fn is_failure(status: &str) -> bool {
    status.starts_with("오류") || status.starts_with("원본 없음") || status.starts_with("접근 권한 필요")
}

/// Port of `PreviewQueue.next` for the batch: the next photo on a job, or the summary when the batch is done.
fn next(cx: &mut AppContext) {
    let Some(directory) = cx.doc.library_folder() else { return };
    let batch = &mut cx.state::<FilesState>().batch;
    if batch.running {
        return;
    }
    let Some(id) = batch.queue.pop_front() else {
        if batch.total > 0 {
            let text = tr_with(
                "미리보기 {0}/{1}장 처리 · 원본 없음/오류 {2}장",
                &[&batch.completed.to_string(), &batch.total.to_string(), &batch.failures.to_string()],
            );
            batch.total = 0;
            cx.status = text;
        }
        return;
    };
    batch.running = true;
    let cancel = batch.cancel.clone();
    let token = cancel.clone();
    let cache = ThumbCache { snapshot: None, directory, id: id as i64, writable: true };
    cx.jobs.spawn(
        "previews",
        "",
        move |_| Ok(crate::worker::rebuild_preview(&cache, true, &cancel)),
        move |cx: &mut AppContext, result: Result<String, String>| {
            let status =
                result.unwrap_or_else(|message| format!("오류: {}", message.trim().lines().last().unwrap_or("")));
            let batch = &mut cx.state::<FilesState>().batch;
            batch.running = false;
            if Arc::ptr_eq(&token, &batch.cancel) {
                batch.completed += 1;
                batch.failures += usize::from(is_failure(&status));
                let shown = match status.split_once(": ") {
                    Some(("오류", rest)) => format!("{}: {rest}", tr("오류")),
                    _ => tr_files(&status),
                };
                cx.status = tr_with(
                    "미리보기 {0}/{1}장 처리 · {2}",
                    &[&batch.completed.to_string(), &batch.total.to_string(), &shown],
                );
                let _ = cx.doc.file_op(FileOp::Invalidate { ids: vec![id] });
            }
            next(cx);
        },
    );
}

/// The status bar's 일괄 미리보기 중단 button, shown while a batch runs (`preview_cancel_button`).
pub fn preview_cancel_button(cx: &mut AppContext, ui: &mut eframe::egui::Ui) {
    if !batch_running(cx) {
        return;
    }
    if crate::widgets::Button::new(&tr("일괄 미리보기 중단")).show(ui).clicked() {
        cx.run("file.stop_previews");
    }
}
