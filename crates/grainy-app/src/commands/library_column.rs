//! The library column's commands: the folder tree's context menu and the − button, and the import's cancel
//! button (luma/app.py `folder_context_menu`, `sync_folder`, `show_parent_folder`, `remove_folder`,
//! `cancel_import`; luma/folder_panel.py `refresh`, `remove`).
//!
//! The folder commands take the folder's path as the invocation's value; `library.remove_folder` without one
//! removes the folder chosen in the tree (the − button).

use std::path::Path;

use grainy_catalog::folders::Platform;
use grainy_library::file_access::{access, Access};

use super::{always, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::confirm::Confirm;
use crate::dialogs::import_errors::Notice;
use crate::session_state::ColumnState;
use crate::text::{tr, tr_with};

/// Tests switch the launch of the file manager off.
pub static LAUNCH_DISABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub const COMMANDS: &[Command] = &[
    Command { id: "library.refresh_folders", label: "폴더 목록 새로 고침", enabled: always, run: refresh },
    Command {
        id: "library.sync_folder", label: "폴더 동기화 · 새 사진 가져오기", enabled: can_sync, run: sync_folder
    },
    Command { id: "library.open_folder", label: "탐색기에서 열기", enabled: always, run: open_folder },
    Command { id: "library.show_parent", label: "상위 폴더 표시", enabled: always, run: show_parent },
    Command {
        id: "library.remove_folder", label: "카탈로그에서 폴더 제거…", enabled: can_remove, run: remove_folder
    },
    Command { id: "library.cancel_import", label: "가져오기 중단", enabled: importing, run: cancel_import },
];

fn path_of(invocation: &Invocation) -> Option<String> {
    invocation.value.as_ref().and_then(|v| v.as_str()).map(str::to_owned)
}

fn can_sync(cx: &AppContext) -> bool {
    !cx.read_only_library() && !cx.jobs.running("import")
}

fn can_remove(cx: &AppContext) -> bool {
    cx.doc.library_folder().is_some()
}

fn importing(cx: &AppContext) -> bool {
    cx.jobs.running("import")
}

/// `FolderPanel.refresh`: the folders are scanned again.
fn refresh(cx: &mut AppContext, _: &Invocation) {
    cx.state::<ColumnState>().rescan();
}

/// Port of `sync_folder`: the folder is imported again (new photos only), unless the system refuses it or it
/// is gone.
fn sync_folder(cx: &mut AppContext, invocation: &Invocation) {
    let Some(path) = path_of(invocation) else { return };
    match access(Path::new(&path), true) {
        Access::Denied => {
            cx.status = tr("원본 폴더에 접근할 권한이 없습니다. 접근을 허용하면 사진이 그대로 돌아옵니다.");
        }
        Access::Missing => {
            cx.status = tr("폴더를 찾을 수 없습니다. 드라이브 연결이나 원본 위치를 확인해 주세요.");
        }
        Access::Present => cx.open_paths(vec![path.into()]),
    }
}

/// `QDesktopServices.openUrl(QUrl.fromLocalFile(path))`: the folder in the system's file manager.
fn open_folder(_: &mut AppContext, invocation: &Invocation) {
    let Some(path) = path_of(invocation) else { return };
    if LAUNCH_DISABLED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    if let Err(error) = std::process::Command::new(program).arg(&path).spawn() {
        log::warn!("cannot open {path}: {error}");
    }
}

/// Port of `show_parent_folder`: the parent becomes a registered folder and is expanded.
fn show_parent(cx: &mut AppContext, invocation: &Invocation) {
    let Some(path) = path_of(invocation) else { return };
    let platform = Platform::native();
    let parent = platform.parent(&path);
    if let Err(message) = cx.doc.register_folder(&parent) {
        cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]);
        return;
    }
    cx.state::<ColumnState>().expanded.insert(platform.path_key(&parent));
    cx.doc.request_refresh(false);
}

/// Port of `remove_folder` and `FolderPanel.remove`: after the question, the folder and the photos below it
/// leave the catalogue (backed up first); files and folders on disk are never touched.
fn remove_folder(cx: &mut AppContext, invocation: &Invocation) {
    let query = cx.doc.query();
    let Some(path) = path_of(invocation).or_else(|| ColumnState::removable(&query)) else { return };
    if Path::new(&path).parent().is_none() {
        return;
    }
    if cx.jobs.running("import") || cx.jobs.running("export") {
        cx.open_dialog(Box::new(Notice::folder_busy()));
        return;
    }
    let count = cx.doc.folder_photo_count(&path);
    let name = Path::new(&path).file_name().map_or_else(|| path.clone(), |n| n.to_string_lossy().into_owned());
    let text = tr_with(
        "\"{0}\" 폴더를 카탈로그에서 제거합니다.\n이 폴더와 하위 폴더의 사진 {1}장이 카탈로그에서 빠지고, 그 사진의 보정·별점·컬렉션 정보도 함께 지워집니다.\n\n원본 파일과 폴더는 그대로 남습니다. 제거 전 카탈로그는 자동으로 백업합니다.",
        &[&name, &count.to_string()],
    );
    cx.open_dialog(Box::new(Confirm::new("remove_folder", tr("폴더 제거"), text, move |cx: &mut AppContext| {
        remove_now(cx, &path);
    })));
}

/// What 예 does: `FolderPanel.remove` after its question.
pub fn remove_now(cx: &mut AppContext, path: &str) {
    let had = cx.doc.current_id();
    match cx.doc.remove_folder(path) {
        Ok(count) => {
            let platform = Platform::native();
            if cx.doc.query().folder.as_deref().is_some_and(|f| platform.contains_folder(path, f)) {
                cx.change_query(|q| q.folder = None);
            }
            let st = cx.state::<ColumnState>();
            st.expanded.retain(|key| !platform.contains_folder(path, key));
            st.rescan();
            cx.status = tr_with(
                "폴더를 카탈로그에서 제거했습니다 · 사진 {0}장 · 원본 파일은 그대로입니다.",
                &[&count.to_string()],
            );
        }
        Err(message) => cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]),
    }
    if cx.doc.current_id() != had {
        cx.photo_changed();
    }
}

/// Port of `cancel_import`: the import stops after the file it is on; photos already added stay.
fn cancel_import(cx: &mut AppContext, _: &Invocation) {
    cx.jobs.cancel_kind("import");
    cx.state::<ColumnState>().cancelling = true;
    cx.status = tr("가져오기를 중단하고 있습니다. 이미 가져온 사진은 유지합니다.");
}

/// What an import's job gives back is shown here (`import_next` at the end of the queue): the list is read
/// again, the status says how it ended, and the unreadable photos are listed in a box.
pub fn import_done(cx: &mut AppContext, result: Result<crate::document::catalog::import::Report, String>) {
    let cancelled = std::mem::take(&mut cx.state::<ColumnState>().cancelling);
    let ensure = cx.doc.current().is_none();
    cx.doc.request_refresh(ensure);
    cx.state::<ColumnState>().rescan();
    match result {
        Ok(report) => {
            cx.status = if cancelled {
                tr("가져오기를 중단했습니다. 이미 가져온 사진은 유지합니다.")
            } else if report.added.is_empty() && report.errors.is_empty() {
                tr("가져올 사진을 찾지 못했습니다.")
            } else {
                tr("사진 가져오기 완료")
            };
            if !cancelled && !report.errors.is_empty() {
                cx.open_dialog(Box::new(Notice::import_errors(&report.errors)));
            }
        }
        Err(message) => cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]),
    }
}
