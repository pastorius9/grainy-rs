//! 피드백 and 구글 드라이브 백업… (work package 31): the commands that open the two windows
//! (luma/feedback.py `show`, luma/manager.py `cloud_backup`).

use super::{always, Command, Invocation};
use crate::context::AppContext;
use crate::dialogs::cloud_backup::CloudBackupDialog;
use crate::dialogs::feedback::FeedbackDialog;

pub const COMMANDS: &[Command] = &[
    Command { id: "feedback.open", label: "피드백 보내기", enabled: always, run: feedback },
    Command {
        id: "file.cloud_backup", label: "구글 드라이브 백업…", enabled: has_library, run: cloud_backup
    },
];

/// The backup needs a library of its own (`w.catalog`).
fn has_library(cx: &AppContext) -> bool {
    cx.doc.library_folder().is_some()
}

fn feedback(cx: &mut AppContext, _: &Invocation) {
    let dialog = FeedbackDialog::new(cx);
    cx.open_dialog(Box::new(dialog));
}

fn cloud_backup(cx: &mut AppContext, _: &Invocation) {
    let dialog = CloudBackupDialog::new(cx);
    cx.open_dialog(Box::new(dialog));
}
