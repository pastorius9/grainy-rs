//! The dust detection buttons of the Healing panel (luma/studio.py `detect_dust`, `detect_dust_batch`): they open
//! the dialogs of [`crate::dialogs::dust`] and [`crate::dialogs::dust_batch`]; an accepted single review becomes
//! one `dust` retouch operation of the open photo.

use serde_json::Value;

use super::{photo_open, writable_library, Command, Invocation};
use crate::commands::sync::SyncState;
use crate::context::AppContext;
use crate::dialogs::dust::{self, DustReview};
use crate::dialogs::dust_batch::{self, DustBatchDialog};
use crate::text::{tr, tr_with};
use crate::tools::healing;

pub const COMMANDS: &[Command] = &[
    Command {
        id: "heal.detect_dust", label: "먼지 자동 감지 · 후보 확인…", enabled: photo_open, run: detect
    },
    Command {
        id: "heal.detect_dust_batch", label: "여러 사진의 먼지 감지…", enabled: batch_enabled, run: batch
    },
];

fn batch_enabled(cx: &AppContext) -> bool {
    writable_library(cx) && cx.doc.metadata_catalog().is_some()
}

const VIDEO: [&str; 5] = ["mp4", "mov", "avi", "mkv", "m4v"];

/// Port of `StudioTools.detect_dust` up to `dialog.exec()`.
fn detect(cx: &mut AppContext, _: &Invocation) {
    let Some(path) = cx.doc.current().map(|p| p.path.clone()) else { return };
    let video = path.extension().and_then(|e| e.to_str()).is_some_and(|e| VIDEO.contains(&e.to_lowercase().as_str()));
    if video {
        cx.set_status(tr("먼지 자동 감지는 사진에서 사용할 수 있습니다."));
        return;
    }
    cx.doc.edit_mut().commit("보정 변경");
    healing::sync(cx);
    healing::set_mode(cx, None);
    dust::open(cx, accepted);
}

/// The rest of `StudioTools.detect_dust` once the dialog accepted: the operation joins the photo's retouch list
/// as one undo step (`먼지 N개 제거`), never synced to other photos.
pub fn accepted(cx: &mut AppContext, review: &DustReview) {
    let Some(operation) = review.operation.clone() else { return };
    if !review.valid_source(cx) {
        cx.set_status(tr("사진이나 원본이 바뀌어 먼지 제거를 적용하지 않았습니다."));
        return;
    }
    let count = operation["spots"].as_array().map_or(0, Vec::len);
    let mut values = cx.settings().value("retouch").as_array().cloned().unwrap_or_default();
    values.push(operation);
    let label = format!("먼지 {count}개 제거");
    cx.state::<SyncState>().local_commits.push(label.clone());
    let edit = cx.doc.edit_mut();
    edit.set("retouch", Value::Array(values));
    edit.commit(&label);
    cx.set_status(tr_with("먼지 {0}개를 제거했습니다. 실행 취소로 되돌릴 수 있습니다.", &[&count.to_string()]));
}

/// Port of `StudioTools.detect_dust_batch`.
fn batch(cx: &mut AppContext, _: &Invocation) {
    cx.doc.edit_mut().commit("보정 변경");
    healing::sync(cx);
    healing::set_mode(cx, None);
    match dust_batch::open(cx) {
        Ok(shared) => cx.open_dialog(Box::new(DustBatchDialog { shared })),
        Err(message) => cx.set_status(tr_with("처리하지 못했습니다: {0}", &[&message])),
    }
}
