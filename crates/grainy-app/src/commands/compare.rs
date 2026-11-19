//! 비교 보기: the compare view (비교, C), the menu items that start it, the survey of the selected photos and the
//! reference windows (luma/compare_view.py, luma/manager.py `compare`, luma/workflow_ui.py `reference`,
//! `second_window`).

use super::{always, has_photos, photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::reference::ReferenceWindow;
use crate::dialogs::survey;
use crate::views::compare::{self, CompareMode};

pub const COMMANDS: &[Command] = &[
    Command { id: "view.compare", label: "비교", enabled: always, run: toggle },
    Command { id: "view.compare_photos", label: "선택 사진 비교", enabled: always, run: photos },
    Command { id: "view.compare_before", label: "현재 사진 전·후 비교", enabled: always, run: before },
    Command { id: "view.survey", label: "선택 사진 모아보기", enabled: has_photos, run: survey_selected },
    Command {
        id: "workflow.reference", label: "현재 사진을 참조 사진으로 고정", enabled: photo_open, run: reference
    },
    Command { id: "workflow.second_window", label: "두 번째 화면 열기", enabled: always, run: second_window },
];

/// The 비교 button and C (`CompareLayout.toggle`).
fn toggle(cx: &mut AppContext, _: &Invocation) {
    compare::toggle(cx);
}

/// 선택 사진 비교 (`compare.start('photos')`).
fn photos(cx: &mut AppContext, _: &Invocation) {
    compare::start(cx, Some(CompareMode::Photos));
}

/// 현재 사진 전·후 비교 (`compare.start('before')`).
fn before(cx: &mut AppContext, _: &Invocation) {
    compare::start(cx, Some(CompareMode::Before));
}

/// 선택 사진 모아보기 (`LibraryManager.compare(12)`).
fn survey_selected(cx: &mut AppContext, _: &Invocation) {
    survey::open(cx, survey::LIMIT, false);
}

/// Port of `WorkflowTools.reference`: a window keeping the open photo as it is now.
fn reference(cx: &mut AppContext, _: &Invocation) {
    cx.doc.edit_mut().commit("보정 변경");
    if let Some(window) = ReferenceWindow::fixed(cx) {
        cx.close_dialog(window.id_str());
        cx.open_dialog(Box::new(window));
    }
}

/// Port of `WorkflowTools.second_window`: a window following the open photo.
fn second_window(cx: &mut AppContext, _: &Invocation) {
    cx.open_dialog(Box::new(ReferenceWindow::following()));
}
