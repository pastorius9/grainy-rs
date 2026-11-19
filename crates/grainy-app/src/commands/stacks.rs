//! Stacks and snapshots (luma/manager.py `stack`, `unstack`, `toggle_stacks`, `snapshot`, `restore_snapshot`;
//! luma/workflow_ui.py `stack_cover`, `stack_move`).
//!
//! A command opens the oracle's boxes and the box's answer calls the matching function below; those are public so
//! that tests, and later features, do what the boxes do without a window. The virtual copy command
//! (`manager.virtual_copy`) lives in `commands/labels.rs` (`cull.virtual_copy`, Ctrl+') and is tested with these.

use super::collections::{fail, selected_ids};
use super::{writable_library, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::snapshots;
use crate::document::catalog::stacks::{SnapshotRow, StackOp, StackReply};

pub const COMMANDS: &[Command] = &[
    Command {
        id: "stacks.stack", label: "선택 사진 스택으로 묶기", enabled: writable_library, run: stack_command
    },
    Command {
        id: "stacks.unstack", label: "선택 사진 스택 해제", enabled: writable_library, run: unstack_command
    },
    Command { id: "stacks.toggle", label: "스택 접어서 보기", enabled: writable_library, run: toggle_command },
    Command {
        id: "stacks.cover", label: "현재 사진을 스택 대표로", enabled: stacked_photo, run: cover_command
    },
    Command { id: "stacks.forward", label: "스택 안에서 앞으로", enabled: stacked_photo, run: forward_command },
    Command { id: "stacks.backward", label: "스택 안에서 뒤로", enabled: stacked_photo, run: backward_command },
    Command {
        id: "snapshots.save", label: "현재 보정 스냅샷 저장", enabled: writable_library, run: save_command
    },
    Command {
        id: "snapshots.restore", label: "스냅샷 불러오기", enabled: writable_library, run: restore_command
    },
];

/// The open photo is in a stack (the context menu's `bool(current and current['stack_id'] is not None)`).
fn stacked_photo(cx: &AppContext) -> bool {
    writable_library(cx) && cx.doc.current().is_some_and(|p| p.stack.is_some())
}

/// Whether a command is a check mark in a menu (스택 접어서 보기).
pub fn checked(cx: &AppContext, id: &str) -> Option<bool> {
    (id == "stacks.toggle").then(|| cx.doc.query().collapsed)
}

/// The title of the toggle in the photo's context menu (`'스택 펼쳐 보기' if manager.collapsed else '스택 접어서 보기'`).
pub fn toggle_title(cx: &AppContext) -> &'static str {
    if cx.doc.query().collapsed {
        "스택 펼쳐 보기"
    } else {
        "스택 접어서 보기"
    }
}

fn op(cx: &mut AppContext, op: StackOp) -> Option<StackReply> {
    match cx.doc.stack_op(op) {
        Ok(reply) => Some(reply),
        Err(message) => {
            fail(cx, &message);
            None
        }
    }
}

// ---- stacks --------------------------------------------------------------------------------------------------

fn stack_command(cx: &mut AppContext, _: &Invocation) {
    let ids = selected_ids(cx);
    stack(cx, ids);
}

/// Port of `manager.stack`: the selected photos become one stack.
pub fn stack(cx: &mut AppContext, ids: Vec<u64>) {
    op(cx, StackOp::Stack { ids });
}

fn unstack_command(cx: &mut AppContext, _: &Invocation) {
    let ids = selected_ids(cx);
    unstack(cx, ids);
}

/// Port of `manager.unstack`: the selected photos leave their stacks.
pub fn unstack(cx: &mut AppContext, ids: Vec<u64>) {
    op(cx, StackOp::Unstack { ids });
}

fn toggle_command(cx: &mut AppContext, _: &Invocation) {
    toggle(cx);
}

/// Port of `toggle_stacks`: the check mark flips and the list is read again (`refresh_lists(ensure_current=True)`).
pub fn toggle(cx: &mut AppContext) {
    cx.change_query(|q| q.collapsed = !q.collapsed);
}

fn cover_command(cx: &mut AppContext, _: &Invocation) {
    cover(cx);
}

/// Port of `stack_cover`: the open photo becomes the first of its stack.
pub fn cover(cx: &mut AppContext) {
    let photo = cx.doc.current_id();
    if photo != 0 {
        op(cx, StackOp::Cover { photo });
    }
}

fn forward_command(cx: &mut AppContext, _: &Invocation) {
    shift(cx, -1);
}

fn backward_command(cx: &mut AppContext, _: &Invocation) {
    shift(cx, 1);
}

/// Port of `stack_move`: the open photo moves `delta` places inside its stack.
pub fn shift(cx: &mut AppContext, delta: i64) {
    let photo = cx.doc.current_id();
    if photo != 0 {
        op(cx, StackOp::Move { photo, delta });
    }
}

// ---- snapshots -----------------------------------------------------------------------------------------------

fn save_command(cx: &mut AppContext, _: &Invocation) {
    cx.open_dialog(Box::new(snapshots::name_box(|cx, name| save(cx, &name))));
}

/// Port of `manager.snapshot` after its name box: nothing for a blank name, otherwise the open photo is committed
/// and its stored edits saved under the stripped name.
pub fn save(cx: &mut AppContext, name: &str) {
    let name = name.trim();
    if name.is_empty() || cx.doc.current().is_none() {
        return;
    }
    op(cx, StackOp::Snapshot { name: name.to_owned() });
}

/// The open photo's snapshots, newest first (`catalog.snapshots(current_id)`).
pub fn list(cx: &mut AppContext) -> Vec<SnapshotRow> {
    match op(cx, StackOp::Snapshots) {
        Some(StackReply::Snapshots(rows)) => rows,
        _ => Vec::new(),
    }
}

fn restore_command(cx: &mut AppContext, _: &Invocation) {
    let rows = list(cx);
    if rows.is_empty() {
        fail(cx, "저장된 스냅샷이 없습니다.");
        return;
    }
    let labels = snapshots::labels(&rows);
    let choice = snapshots::choice_box(labels, move |cx, index| {
        if let Some(row) = rows.get(index) {
            restore(cx, row);
        }
    });
    cx.open_dialog(Box::new(choice));
}

/// Port of `manager.restore_snapshot` after the choice: the open photo's edits are committed, then the snapshot's
/// settings (normalized) replace them as one step labelled 스냅샷 복원.
pub fn restore(cx: &mut AppContext, row: &SnapshotRow) {
    cx.doc.edit_mut().replace(row.settings.clone(), "스냅샷 복원");
}
