//! Merge selected originals into a user-named, new TIFF and import it back into the originating document.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use grainy_engine::merge::{merge_to_tiff, Mode};
use serde_json::json;

use super::{Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::maintenance::{PathWait, Pick};
use crate::document::catalog::import;
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "photo.merge_hdr", label: "HDR 노출 병합…", enabled, run: hdr },
    Command { id: "photo.merge_panorama", label: "평면 파노라마 병합…", enabled, run: panorama },
];

fn enabled(cx: &AppContext) -> bool {
    !cx.read_only_library() && !cx.jobs.running("photo_merge") && cx.doc.selected_indices().len() >= 2
}

fn hdr(cx: &mut AppContext, invocation: &Invocation) {
    choose(cx, invocation, Mode::Hdr);
}
fn panorama(cx: &mut AppContext, invocation: &Invocation) {
    choose(cx, invocation, Mode::Panorama);
}

fn choose(cx: &mut AppContext, invocation: &Invocation, mode: Mode) {
    let paths: Vec<_> =
        cx.doc.selected_indices().into_iter().filter_map(|i| cx.doc.photos().get(i).map(|p| p.path.clone())).collect();
    let Some(first) = paths.first() else { return };
    if let Some(path) = invocation.value.as_ref().and_then(|v| v.as_str()) {
        start(cx, paths, PathBuf::from(path), mode);
        return;
    }
    let name = first.file_stem().unwrap_or_default().to_string_lossy();
    let (label, suffix) =
        if mode == Mode::Hdr { ("HDR 노출 병합…", "HDR") } else { ("평면 파노라마 병합…", "Panorama") };
    let pick = Pick::Save(format!("{name}-{suffix}.tif"), ("TIFF".into(), vec!["tif", "tiff"]));
    cx.open_dialog(Box::new(PathWait::open(
        tr(label),
        pick,
        first.parent().map(Path::to_path_buf),
        move |cx, path| start(cx, paths, path, mode),
    )));
}

fn start(cx: &mut AppContext, paths: Vec<PathBuf>, output: PathBuf, mode: Mode) {
    let total = 2 * paths.len();
    save_derived(cx, "photo_merge", tr("사진 병합"), output, mode == Mode::Hdr, move |handle, path| {
        let progress = |step| handle.progress(step, total, "");
        merge_to_tiff(&paths, path, mode, handle.cancel_flag(), &progress).map(|_| ())
    });
}

fn add_to_catalog(library: &Path, output: &Path, hdr: bool) -> Result<(), String> {
    let catalog = grainy_catalog::Catalog::new(library).map_err(|e| e.to_string())?;
    let roots = output.parent().map(Path::to_path_buf).into_iter().collect::<Vec<_>>();
    let file = output.to_path_buf();
    let report =
        import::import_files(&catalog, std::slice::from_ref(&file), &roots, &AtomicBool::new(false), &|_, _, _| {})?;
    if !report.errors.is_empty() {
        return Err(report.errors.join("\n"));
    }
    if hdr {
        if let Some(&id) = report.added.first() {
            let mut settings = grainy_core::defaults();
            settings.set("hdr", json!(true));
            catalog.edit(id, &settings, "HDR").map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn import_error(path: &Path, error: &str) -> String {
    tr_with("TIFF 저장 완료 ({0}). 가져오기 실패: {1}", &[&path.display().to_string(), error])
}

/// Save a newly generated photo on a background job and add it to the originating session or catalog.
pub(crate) fn save_derived(
    cx: &mut AppContext,
    kind: &'static str,
    label: String,
    output: PathBuf,
    hdr: bool,
    work: impl FnOnce(&crate::jobs::JobHandle, &Path) -> Result<(), String> + Send + 'static,
) {
    if let Err(e) = crate::paths::writable(&output) {
        cx.status = tr_with("처리하지 못했습니다: {0}", &[&e.to_string()]);
        return;
    }
    let library = cx.doc.library_folder();
    let session = library.is_none();
    let saved = output.clone();
    cx.spawn_document_job(
        kind,
        label,
        move |handle| {
            if let Err(error) = work(handle, &output) {
                if handle.cancelled() {
                    return Ok(false);
                }
                return Err(error);
            }
            // Once the file is committed, register it even if cancellation arrived during the final write.
            if let Some(library) = library {
                add_to_catalog(&library, &output, hdr).map_err(|e| import_error(&output, &e))?;
            }
            Ok(true)
        },
        move |cx, result| {
            if matches!(result, Ok(false)) {
                cx.status.clear();
                return;
            }
            cx.doc.request_refresh(false);
            let result = result.and_then(|_| {
                if session {
                    cx.doc.add_files(vec![saved.clone()], &[]).map_err(|e| import_error(&saved, &e))?;
                    if let Some(i) = cx.doc.photos().iter().position(|p| p.path == saved) {
                        cx.select_photo(i);
                        if hdr {
                            let edit = cx.doc.edit_mut();
                            edit.set("hdr", json!(true));
                            edit.commit("HDR");
                        }
                    }
                }
                Ok(())
            });
            cx.status = match result {
                Ok(()) => tr_with("새 사진을 저장했습니다: {0}", &[&saved.display().to_string()]),
                Err(e) => tr_with("처리하지 못했습니다: {0}", &[&e]),
            };
        },
    );
}

#[cfg(test)]
#[path = "merge_tests.rs"]
mod tests;
