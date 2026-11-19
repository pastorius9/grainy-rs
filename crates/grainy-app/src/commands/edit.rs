//! 편집: undo, redo, copy and paste of edits, reset (luma/app.py `undo`, `redo`, `copy_edits`, `paste_edits`,
//! `reset_edits`).

use super::{photo_open, Command, Invocation};
use crate::context::AppContext;
use crate::text::tr;

pub const COMMANDS: &[Command] = &[
    Command { id: "develop.undo", label: "실행 취소", enabled: photo_open, run: undo },
    Command { id: "develop.redo", label: "다시 실행", enabled: photo_open, run: redo },
    Command { id: "develop.copy_edits", label: "보정 복사", enabled: photo_open, run: copy },
    Command { id: "develop.paste_edits", label: "보정 붙여넣기", enabled: photo_open, run: paste },
    Command { id: "develop.reset", label: "보정 초기화", enabled: photo_open, run: reset },
];

fn undo(cx: &mut AppContext, _: &Invocation) {
    cx.doc.edit_mut().undo();
    cx.before = false;
}

fn redo(cx: &mut AppContext, _: &Invocation) {
    cx.doc.edit_mut().redo();
    cx.before = false;
}

/// Port of `copy_edits`.
fn copy(cx: &mut AppContext, _: &Invocation) {
    if cx.doc.copy() {
        cx.status = tr("현재 보정값을 복사했습니다.");
    }
}

/// Port of `paste_edits` for the selected photos (`sync::paste`: the copy set of 복사·동기화 항목).
fn paste(cx: &mut AppContext, _: &Invocation) {
    super::sync::paste(cx);
}

/// Port of `reset_edits` (without its confirmation).
fn reset(cx: &mut AppContext, _: &Invocation) {
    cx.doc.edit_mut().reset();
}
