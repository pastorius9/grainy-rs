//! 파일 관리: import (add in place), opening a library, opening the Python Grainy's library read-only,
//! removing photos from the library (luma/app.py `import_files`, `import_folder`, `import_paths`, `import_next`;
//! luma/manager.py `remove`). The export commands are in `export.rs`.

use std::path::{Path, PathBuf};

use super::{always, has_photos, writable_library, Command, Invocation};
use crate::context::{AppContext, Mode};
use crate::dialogs::confirm::Confirm;
use crate::dialogs::native::Purpose;
use crate::document::catalog::{import, CatalogDocument};
use crate::document::Kind;
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "file.import", label: "사진 가져오기", enabled: can_import, run: import_files },
    Command {
        id: "file.import_folder",
        label: "사진 폴더 가져오기 (하위 폴더 포함)",
        enabled: can_import,
        run: import_folder,
    },
    Command {
        id: "file.open_library", label: "라이브러리 폴더 열기…", enabled: always, run: choose_library
    },
    Command {
        id: "file.open_grainy_library",
        label: "Grainy 라이브러리 읽기 전용으로 열기",
        enabled: always,
        run: open_grainy_library,
    },
    Command { id: "file.remove", label: "카탈로그에서만 제거", enabled: removable, run: remove },
];

fn can_import(cx: &AppContext) -> bool {
    !cx.read_only_library() && !cx.jobs.running("import")
}

fn removable(cx: &AppContext) -> bool {
    has_photos(cx) && (writable_library(cx) || matches!(cx.doc.kind(), Kind::Session))
}

fn import_files(cx: &mut AppContext, _: &Invocation) {
    cx.choose(Purpose::ImportFiles);
}

fn import_folder(cx: &mut AppContext, _: &Invocation) {
    cx.choose(Purpose::ImportFolder);
}

/// Port of `import_paths`: the files are found and added on a background job. A library imports them in place
/// (the catalogue records where they are; nothing is copied); the session only lists them.
pub fn import_paths(cx: &mut AppContext, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    if cx.read_only_library() {
        cx.status = tr("읽기 전용 라이브러리에는 사진을 가져올 수 없습니다.");
        return;
    }
    match cx.doc.library_folder() {
        None => {
            cx.spawn_document_job(
                "import",
                tr("폴더를 찾는 중…"),
                move |_| Ok(crate::session::photo_files(&paths)),
                |cx: &mut AppContext, result: Result<Vec<PathBuf>, String>| {
                    let had = cx.doc.current_id();
                    let added = result.and_then(|files| cx.doc.add_files(files, &[]));
                    cx.status = match added {
                        Ok(0) => tr("가져올 사진을 찾지 못했습니다."),
                        Ok(n) => tr_with("사진 {0}장을 가져왔습니다.", &[&n.to_string()]),
                        Err(message) => tr_with("처리하지 못했습니다: {0}", &[&message]),
                    };
                    if cx.doc.current_id() != had {
                        cx.photo_changed();
                    }
                },
            );
        }
        Some(folder) => {
            cx.status = tr("폴더를 찾는 중…");
            cx.spawn_document_job(
                "import",
                tr("사진 가져오기"),
                move |handle| {
                    let (files, roots) = import::scan(&paths, handle.cancel_flag());
                    let catalog = grainy_catalog::Catalog::new(&folder).map_err(|e| e.to_string())?;
                    let progress = |done: usize, total: usize, name: &str| {
                        let text = if name.is_empty() {
                            String::new()
                        } else {
                            tr_with("가져오는 중: {0}  ·  남은 사진 {1}장", &[name, &(total - done).to_string()])
                        };
                        handle.progress(done, total, text);
                    };
                    import::import_files(&catalog, &files, &roots, handle.cancel_flag(), &progress)
                },
                super::library_column::import_done,
            );
        }
    }
}

fn choose_library(cx: &mut AppContext, _: &Invocation) {
    cx.choose(Purpose::OpenLibrary);
}

#[derive(Default)]
struct OpenRequest(u64);

fn next_open(cx: &mut AppContext) -> u64 {
    let request = cx.state::<OpenRequest>();
    request.0 += 1;
    request.0
}

/// Opens a library folder for writing on a job; the Python Grainy's folders open read-only instead.
pub fn open_library(cx: &mut AppContext, folder: &Path) {
    let locations = crate::paths::Locations::current();
    if crate::paths::is_oracle_folder(folder, &locations) {
        open_read_only(cx, folder.to_path_buf());
        return;
    }
    let folder = folder.to_path_buf();
    cx.status = tr("불러오는 중…");
    let ticket = next_open(cx);
    cx.jobs.spawn(
        "open",
        tr("라이브러리 폴더 열기…"),
        move |_| CatalogDocument::open(&folder),
        move |cx: &mut AppContext, result: Result<CatalogDocument, String>| match result {
            _ if cx.state::<OpenRequest>().0 != ticket => {}
            Ok(doc) => {
                if !cx.set_document(Box::new(doc)) {
                    return;
                }
                cx.set_mode(Mode::Library);
                cx.status = tr("준비됨");
            }
            Err(message) => cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]),
        },
    );
}

/// Opens the Python Grainy's library (`%LOCALAPPDATA%\Luma\Library`) read-only.
fn open_grainy_library(cx: &mut AppContext, _: &Invocation) {
    let folder = crate::paths::Locations::current().oracle_library();
    if !folder.join("catalog.sqlite").is_file() {
        cx.status = tr("Grainy 라이브러리를 찾지 못했습니다.");
        return;
    }
    open_read_only(cx, folder);
}

/// Opens `folder` read-only: listings come from a snapshot copy in the Rust app's data folder.
pub fn open_read_only(cx: &mut AppContext, folder: PathBuf) {
    let scratch = cx
        .data_folder
        .clone()
        .unwrap_or_else(|| crate::paths::Locations::current().data_folder())
        .join("read-only snapshot");
    cx.status = tr("불러오는 중…");
    let ticket = next_open(cx);
    cx.jobs.spawn(
        "open",
        tr("읽기 전용"),
        move |_| CatalogDocument::open_read_only(&folder, &scratch),
        move |cx: &mut AppContext, result: Result<CatalogDocument, String>| match result {
            _ if cx.state::<OpenRequest>().0 != ticket => {}
            Ok(doc) => {
                if !cx.set_document(Box::new(doc)) {
                    return;
                }
                cx.set_mode(Mode::Library);
                cx.status = tr("읽기 전용 · 보정은 이 창에서만 유지되고 저장되지 않습니다.");
            }
            Err(message) => cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]),
        },
    );
}

/// Port of `manager.remove(False)`: the selected photos leave the library after a question; files stay.
fn remove(cx: &mut AppContext, _: &Invocation) {
    let count = cx.doc.selected_indices().len();
    if count == 0 {
        return;
    }
    let text = format!(
        "{}\n{}",
        tr_with("선택한 {0}장을 카탈로그에서 제거합니다.", &[&count.to_string()]),
        tr("원본 파일은 그대로 남습니다.")
    );
    cx.open_dialog(Box::new(Confirm::new("remove", tr("사진 제거"), text, |cx: &mut AppContext| {
        let had = cx.doc.current_id();
        match cx.doc.remove_selected() {
            Ok(n) => cx.status = tr_with("{0}장", &[&n.to_string()]),
            Err(message) => cx.status = tr_with("처리하지 못했습니다: {0}", &[&message]),
        }
        if cx.doc.current_id() != had {
            cx.photo_changed();
        }
    })));
}
