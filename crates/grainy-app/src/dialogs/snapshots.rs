//! The two boxes of the snapshot commands (luma/manager.py `snapshot` and `restore_snapshot`): the name box
//! `QInputDialog.getText(w, tr('스냅샷'), tr('이름'))` and the choice box `QInputDialog.getItem(w, tr('스냅샷'),
//! tr('복원할 보정'), labels, editable=False)` whose rows are `{id} · {name}`. Both are the collections package's
//! generic input boxes with the oracle's texts.

use super::filter::{ChoiceBox, TextBox};
use crate::context::AppContext;
use crate::document::catalog::stacks::SnapshotRow;
use crate::text::tr;

/// The id of the name box.
pub const NAME_BOX: &str = "snapshots_name";
/// The id of the choice box.
pub const CHOICE_BOX: &str = "snapshots_choice";

/// The name box; `on_accept` gets the text as typed.
pub fn name_box(on_accept: impl FnOnce(&mut AppContext, String) + 'static) -> TextBox {
    TextBox::new(NAME_BOX, tr("스냅샷"), tr("이름"), "", on_accept)
}

/// The rows of the choice box (`f'{s["id"]} · {s["name"]}'`).
pub fn labels(rows: &[SnapshotRow]) -> Vec<String> {
    rows.iter().map(|s| format!("{} · {}", s.id, s.name)).collect()
}

/// The choice box over the rows' [`labels`]; `on_accept` gets the chosen row's index.
pub fn choice_box(labels: Vec<String>, on_accept: impl FnOnce(&mut AppContext, usize) + 'static) -> ChoiceBox {
    ChoiceBox::new(CHOICE_BOX, tr("스냅샷"), tr("복원할 보정"), labels, on_accept)
}
