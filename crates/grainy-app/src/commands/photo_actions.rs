//! The photo actions of the context menu (APP_PLAN package 14; luma/photo_actions.py `edit_selected`,
//! `transform_selected`, `reset_selected`; luma/workflow_ui.py `camera_orientation`; luma/app.py
//! `open_library_photo`, `show_file`).
//!
//! The oracle writes the selected photos through `LibraryManager.edit_photo` (one history row and one undo step
//! per photo: turns, flips, 흑백 / 컬러 전환, reset; [`Document::edit_photos`]) and 카메라 방향으로 복원 through
//! `WorkflowTools.apply_edits` (`command_batch.apply` without a batch record, panels switched on as in the editor;
//! [`Document::edit_photos_batch_own`]: the values come from the photo's own settings, so the RAW colour settings
//! are not fitted to the file again). Both are done here; the functions are public so that tests do what the menu
//! does without a window. The folder is opened through a [`FolderOpener`] so that tests never start the file
//! manager.

use std::path::Path;

use grainy_core::Settings;
use serde_json::Value;

use super::collections::selected_ids;
use super::{has_photos, Command, Invocation};
use crate::context::{AppContext, Mode, Zoom};
use crate::dialogs::dust::thousands;
use crate::dialogs::import_errors::Notice;
use crate::text::{tr, tr_with};

pub const COMMANDS: &[Command] = &[
    Command { id: "photo.develop", label: "현상에서 크게 보기", enabled: super::photo_open, run: develop },
    Command {
        id: "photo.rotate_right", label: "시계 방향으로 90° 회전", enabled: has_photos, run: rotate_right
    },
    Command {
        id: "photo.rotate_left", label: "반시계 방향으로 90° 회전", enabled: has_photos, run: rotate_left
    },
    Command { id: "photo.flip", label: "좌우 반전", enabled: has_photos, run: flip },
    Command { id: "photo.flip_vertical", label: "상하 반전", enabled: has_photos, run: flip_vertical },
    Command {
        id: "photo.camera_orientation",
        label: "카메라 방향으로 복원",
        enabled: has_photos,
        run: camera_orientation_command,
    },
    Command { id: "photo.monochrome", label: "흑백 / 컬러 전환", enabled: has_photos, run: monochrome },
    Command { id: "photo.reset", label: "선택 사진 보정 초기화", enabled: has_photos, run: reset_command },
    Command {
        id: "photo.show_folder",
        label: crate::system_open::SHOW_FOLDER_IN_FILE_MANAGER,
        enabled: super::photo_open,
        run: show_folder_command,
    },
];

/// Opens a folder in the system's file manager (`QDesktopServices.openUrl`). Tests put a recorder here.
pub trait FolderOpener: Send {
    /// Shows `folder`.
    fn open(&mut self, folder: &Path);
}

/// What the feature keeps between frames.
#[derive(Default)]
pub struct PhotoActions {
    /// The folder opener; `None` is the system's (`library.open_folder`, which tests switch off with
    /// `LAUNCH_DISABLED`).
    pub opener: Option<Box<dyn FolderOpener>>,
}

/// Port of `manager.run`'s box when an action raises: `QMessageBox.warning(w, tr('작업을 완료하지 못했습니다'), tr(str(e)))`.
fn failed(cx: &mut AppContext, message: &str) {
    cx.open_dialog(Box::new(Notice {
        id: "photo_actions_error",
        title: tr("작업을 완료하지 못했습니다"),
        text: tr(message),
    }));
}

/// `MainWindow.commit()`: the pending change becomes a step and is stored.
fn commit_pending(cx: &mut AppContext) {
    cx.doc.edit_mut().commit("보정 변경");
    cx.flush();
}

/// `w.end_crop()`: the crop frame closes.
fn end_crop(cx: &mut AppContext) {
    if crate::tools::crop::is_open(cx) {
        crate::tools::crop::with_session(cx, |s, _| s.frame.set_crop_mode(false, None));
    }
}

/// The end of `edit_selected`: the crop frame closes, the original comparison ends, `fit_photo` when asked.
fn finish(cx: &mut AppContext, fit: bool) {
    end_crop(cx);
    cx.before = false;
    if fit {
        cx.set_zoom(Zoom::Fit);
    }
}

/// Port of `photo_actions.edit_selected`: `change` is applied to the stored settings of every selected photo, each
/// written as one history row `label` with an undo step of its own. Returns how many photos.
pub fn edit_selected(
    cx: &mut AppContext,
    change: impl Fn(&mut Settings),
    label: &str,
    fit: bool,
) -> Result<usize, String> {
    commit_pending(cx);
    let ids = selected_ids(cx);
    for id in &ids {
        let mut value = cx.doc.stored_settings_of(*id)?;
        change(&mut value);
        cx.doc.edit_photos(&[*id], &[value], label)?;
    }
    finish(cx, fit);
    cx.status =
        tr_with("{0}장의 사진에 적용했습니다. 사진별 실행 취소로 되돌릴 수 있습니다.", &[&ids.len().to_string()]);
    Ok(ids.len())
}

/// Port of `photo_actions.transform_selected`: `turns` quarter turns (clearing the crop, which belongs to the
/// rotated frame) and/or a flip, label 사진 회전 or 사진 반전.
pub fn transform_selected(cx: &mut AppContext, turns: Option<i64>, flip: bool) -> Result<usize, String> {
    let change = |values: &mut Settings| {
        if let Some(turns) = turns {
            let rotation = (values.i64("rotation") + turns).rem_euclid(4);
            values.set("rotation", rotation);
            values.set("crop", Value::Null);
        }
        if flip {
            let flipped = !values.bool("flip");
            values.set("flip", flipped);
        }
    };
    edit_selected(cx, change, if flip { "사진 반전" } else { "사진 회전" }, true)
}

/// Port of `photo_actions.reset_selected`: every selected photo back to the defaults, label 보정 초기화 (the
/// alignment analyses that are still running are made stale: `upright_generation += 1`).
pub fn reset_selected(cx: &mut AppContext) -> Result<usize, String> {
    cx.state::<crate::panels::transform::UprightState>().generation += 1;
    let count = super::sync::reset_photos(cx)?;
    finish(cx, true);
    Ok(count)
}

/// Port of the 흑백으로 전환 / 컬러로 전환 items: `monochrome` on the selected photos, label 흑백 / 컬러 전환.
pub fn set_monochrome(cx: &mut AppContext, on: bool) -> Result<usize, String> {
    edit_selected(
        cx,
        |values| {
            values.set("monochrome", on);
        },
        "흑백 / 컬러 전환",
        false,
    )
}

/// Port of `WorkflowTools.camera_orientation`: the rotated or flipped photos of the selection lose rotation, flip
/// and crop (the crop is defined in the rotated frame), one batch labelled 카메라 방향 복원 (panels the changes
/// belong to are switched on), then the view fits. Returns the changed ids.
pub fn camera_orientation(cx: &mut AppContext) -> Result<Vec<u64>, String> {
    commit_pending(cx);
    let mut ids = Vec::new();
    let mut values = Vec::new();
    for id in selected_ids(cx) {
        let stored = cx.doc.stored_settings_of(id)?;
        if stored.i64("rotation") == 0 && !stored.bool("flip") {
            continue;
        }
        let mut value = stored.clone();
        value.set("rotation", 0);
        value.set("flip", false);
        value.set("crop", Value::Null);
        let changed = grainy_core::panel_keys::changed_keys(&stored.0, &value.0);
        grainy_core::panel_keys::switch_on(&mut value.0, &changed);
        ids.push(id);
        values.push(value);
    }
    let mut changed = Vec::new();
    if !ids.is_empty() {
        changed = cx.doc.edit_photos_batch_own(&ids, &values, "카메라 방향 복원")?;
        if changed.contains(&cx.doc.current_id()) {
            cx.before = false;
        }
        cx.status = tr_with("{0} · {1}장 변경", &[&tr("카메라 방향 복원"), &thousands(changed.len())]);
    }
    cx.set_zoom(Zoom::Fit);
    Ok(changed)
}

/// Port of `open_library_photo` for the open photo: the photo opens in the Develop screen, fitted.
pub fn open_develop(cx: &mut AppContext) {
    cx.set_zoom(Zoom::Fit);
    cx.set_mode(Mode::Develop);
}

/// Port of `show_file`: the open photo's folder in the file manager.
pub fn show_folder(cx: &mut AppContext) {
    let Some(folder) = cx.doc.current().and_then(|p| p.path.parent().map(Path::to_path_buf)) else { return };
    let opener = cx.state::<PhotoActions>().opener.take();
    match opener {
        Some(mut opener) => {
            opener.open(&folder);
            cx.state::<PhotoActions>().opener = Some(opener);
        }
        None => {
            let folder = folder.to_string_lossy().into_owned();
            cx.run_with("library.open_folder", &Invocation::value(folder));
        }
    }
}

fn develop(cx: &mut AppContext, _: &Invocation) {
    open_develop(cx);
}

/// `manager.run(callback)`: an error is a message box.
fn report<T>(cx: &mut AppContext, result: Result<T, String>) {
    if let Err(message) = result {
        failed(cx, &message);
    }
}

fn rotate_right(cx: &mut AppContext, _: &Invocation) {
    let result = transform_selected(cx, Some(1), false);
    report(cx, result);
}

fn rotate_left(cx: &mut AppContext, _: &Invocation) {
    let result = transform_selected(cx, Some(-1), false);
    report(cx, result);
}

fn flip(cx: &mut AppContext, _: &Invocation) {
    let result = transform_selected(cx, None, true);
    report(cx, result);
}

fn flip_vertical(cx: &mut AppContext, _: &Invocation) {
    let result = transform_selected(cx, Some(2), true);
    report(cx, result);
}

fn camera_orientation_command(cx: &mut AppContext, _: &Invocation) {
    let result = camera_orientation(cx);
    report(cx, result);
}

/// The value is `true` for 흑백으로 전환 and `false` for 컬러로 전환.
fn monochrome(cx: &mut AppContext, invocation: &Invocation) {
    let on = invocation.value.as_ref().and_then(Value::as_bool).unwrap_or(true);
    let result = set_monochrome(cx, on);
    report(cx, result);
}

fn reset_command(cx: &mut AppContext, _: &Invocation) {
    let result = reset_selected(cx);
    report(cx, result);
}

fn show_folder_command(cx: &mut AppContext, _: &Invocation) {
    show_folder(cx);
}
