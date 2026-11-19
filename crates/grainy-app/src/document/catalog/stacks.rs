//! Stacks and snapshots on the catalogue (luma/manager.py `stack`, `unstack`, `snapshot`, `restore_snapshot`;
//! luma/workflow_ui.py `stack_cover`, `stack_move`), done with the oracle's own statements: `Catalog.stack`,
//! `Catalog.update_many(…, 'stack_id', None)`, `workflow_data.stack_order`, `Catalog.snapshot` and
//! `Catalog.snapshots`.
//!
//! The oracle asks for `refresh_lists()` (without `ensure_current`) after each stack change; the same refresh is
//! requested here. `스택 접어서 보기` is the document's [`Query::collapsed`](crate::document::Query).

use grainy_catalog::workflow;
use grainy_core::Settings;
use serde_json::Value;

use super::{err, settings_value, CatalogDocument};
use crate::document::Document;

/// One operation.
#[derive(Debug, Clone, PartialEq)]
pub enum StackOp {
    /// `manager.stack`: the ids become one stack (its id is the smallest id).
    Stack { ids: Vec<u64> },
    /// `manager.unstack`: the ids leave their stacks.
    Unstack { ids: Vec<u64> },
    /// `stack_cover`: the photo becomes the first of its stack.
    Cover { photo: u64 },
    /// `stack_move`: the photo moves `delta` places inside its stack (nothing when it is not stacked).
    Move { photo: u64, delta: i64 },
    /// `workflow_data.stack_order(catalog, photo)`: the members in their saved order.
    Order { photo: u64 },
    /// `manager.snapshot` after its name box: commits the open photo, then saves its stored edits under `name`.
    Snapshot { name: String },
    /// `catalog.snapshots(current_id)`: the open photo's snapshots, newest first.
    Snapshots,
}

/// A row of `catalog.snapshots`.
#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotRow {
    pub id: i64,
    pub name: String,
    /// `created` (an ISO time).
    pub created: String,
    pub settings: Settings,
}

/// What an operation answers.
#[derive(Debug, Clone, PartialEq)]
pub enum StackReply {
    Done,
    /// `Order`: the photo ids of the stack, cover first (empty when the photo is not stacked).
    Order(Vec<u64>),
    /// `Snapshots`.
    Snapshots(Vec<SnapshotRow>),
}

fn ids(list: &[u64]) -> Vec<i64> {
    list.iter().map(|i| *i as i64).collect()
}

/// Runs `op` on the catalogue document (called by `Document::stack_op`).
pub(super) fn apply(doc: &mut CatalogDocument, op: StackOp) -> Result<StackReply, String> {
    let writes = !matches!(op, StackOp::Order { .. } | StackOp::Snapshots);
    if writes && doc.read_only {
        return Err("read-only".into());
    }
    match op {
        StackOp::Stack { ids: members } => {
            doc.catalog.stack(&ids(&members)).map_err(err)?;
            doc.request_refresh(false);
            Ok(StackReply::Done)
        }
        StackOp::Unstack { ids: members } => {
            doc.catalog.update_many(&ids(&members), "stack_id", &Value::Null).map_err(err)?;
            doc.request_refresh(false);
            Ok(StackReply::Done)
        }
        StackOp::Cover { photo } => {
            workflow::stack_order(&doc.catalog, photo as i64, Some(0)).map_err(err)?;
            doc.request_refresh(false);
            Ok(StackReply::Done)
        }
        StackOp::Move { photo, delta } => {
            let order = workflow::stack_order(&doc.catalog, photo as i64, None).map_err(err)?;
            if let Some(index) = order.iter().position(|i| *i == photo as i64) {
                workflow::stack_order(&doc.catalog, photo as i64, Some(index as i64 + delta)).map_err(err)?;
                doc.request_refresh(false);
            }
            Ok(StackReply::Done)
        }
        StackOp::Order { photo } => {
            let order = workflow::stack_order(&doc.catalog, photo as i64, None).map_err(err)?;
            Ok(StackReply::Order(order.into_iter().map(|i| i as u64).collect()))
        }
        StackOp::Snapshot { name } => {
            let Some(id) = doc.open.as_ref().map(|o| o.id) else { return Ok(StackReply::Done) };
            doc.edit.commit("보정 변경");
            Document::flush(doc)?;
            doc.catalog.snapshot(id as i64, &name).map_err(err)?;
            Ok(StackReply::Done)
        }
        StackOp::Snapshots => {
            let Some(id) = doc.open.as_ref().map(|o| o.id) else { return Ok(StackReply::Snapshots(Vec::new())) };
            let rows = doc.catalog.snapshots(id as i64).map_err(err)?;
            Ok(StackReply::Snapshots(
                rows.iter()
                    .map(|r| SnapshotRow {
                        id: r["id"].as_i64().unwrap_or(0),
                        name: r["name"].as_str().unwrap_or("").to_owned(),
                        created: r["created"].as_str().unwrap_or("").to_owned(),
                        settings: settings_value(&r["settings"]),
                    })
                    .collect(),
            ))
        }
    }
}
