//! The collection operations of the library column (luma/manager.py `create_collection`, `add_to_collection`,
//! `remove_collection`, `edit_collection`, `remove_from_collection`; luma/workflow_ui.py `quick_toggle`,
//! `quick_show`, `containing_collection`), done on the catalogue with the oracle's own statements.
//!
//! The "selected collection" of the oracle's manager (`manager.collection`) is the document's
//! [`Query::collection`](crate::document::Query); the operations that change it (a new collection becomes the
//! selected one, a removed one leaves) do it here, together with the refresh of the list the oracle asks for
//! (`refresh_lists(ensure_current=True)` or without).

use std::path::Path;
use std::time::Duration;

use grainy_catalog::json::dumps;
use grainy_catalog::rusqlite::{self, params, Connection};
use grainy_catalog::workflow;
use serde_json::Value;

use super::{err, CatalogDocument};
use crate::document::Collection;

/// One operation.
#[derive(Debug, Clone, PartialEq)]
pub enum CollectionOp {
    /// `create_collection` after its boxes: the name (already stripped), the rules of a smart collection (`None`
    /// for an ordinary one, which takes `members`), under the selected collection; the new one is selected.
    Create { name: String, rules: Option<Value>, members: Vec<u64> },
    /// `add_to_collection` after the choice: `collection_add`.
    AddMembers { collection: i64, ids: Vec<u64> },
    /// `remove_collection`: the selected collection goes (its children stay, without a parent).
    RemoveSelected,
    /// `edit_collection` after its boxes: the selected collection's name and (smart ones) rules.
    Edit { name: String, rules: Option<Value> },
    /// `remove_from_collection`: the ids leave the selected collection.
    RemoveMembers { ids: Vec<u64> },
    /// The name and rules of a collection (the boxes of `edit_collection` start from them).
    Info { collection: i64 },
    /// The collections (read fresh, by name) the photo belongs to (`containing_collection`).
    Containing { photo: u64 },
    /// The collections, read fresh, by name (`catalog.collections()`).
    List,
    /// `toggle_quick`: the ids join the quick collection, or leave it when they all are in it.
    ToggleQuick { ids: Vec<u64>, label: String },
    /// `quick_collection`: the quick collection id (made when it is missing).
    Quick { label: String },
}

/// What an operation answers.
#[derive(Debug, Clone, PartialEq)]
pub enum CollectionReply {
    Done,
    /// A collection id (the new one of `Create`, the quick collection).
    Id(i64),
    /// `Info`: the name and the rules text parsed (`None` for an ordinary collection).
    Info {
        name: String,
        rules: Option<Value>,
    },
    /// `Containing` and `List`.
    Collections(Vec<Collection>),
    /// `ToggleQuick`: the quick collection and whether the photos were added (not removed).
    Toggled {
        id: i64,
        added: bool,
    },
}

fn ids(list: &[u64]) -> Vec<i64> {
    list.iter().map(|i| *i as i64).collect()
}

fn collection_of(row: &Value) -> Collection {
    Collection {
        id: row["id"].as_i64().unwrap_or(0),
        name: row["name"].as_str().unwrap_or("").to_owned(),
        parent: row["parent_id"].as_i64(),
        smart: !row["rules"].is_null(),
    }
}

/// A second connection for the two statements `Catalog` has no method for (the oracle runs them on its own
/// connection: `UPDATE collections` and `DELETE FROM collection_members`).
fn write(folder: &Path, work: impl FnOnce(&Connection) -> rusqlite::Result<()>) -> Result<(), String> {
    let connection = Connection::open(folder.join("catalog.sqlite")).map_err(err)?;
    connection.busy_timeout(Duration::from_secs(10)).map_err(err)?;
    work(&connection).map_err(err)
}

/// Runs `op` on the catalogue document (called by `Document::collection_op`).
pub(super) fn apply(doc: &mut CatalogDocument, op: CollectionOp) -> Result<CollectionReply, String> {
    let writes = !matches!(op, CollectionOp::Info { .. } | CollectionOp::Containing { .. } | CollectionOp::List);
    if writes && doc.read_only {
        return Err("read-only".into());
    }
    let catalog = &doc.catalog;
    match op {
        CollectionOp::Create { name, rules, members } => {
            let parent = doc.query.collection;
            let id = catalog.add_collection(&name, rules.as_ref(), parent).map_err(err)?;
            if rules.is_none() {
                catalog.collection_add(id, &ids(&members)).map_err(err)?;
            }
            doc.query.collection = Some(id);
            doc.request_refresh(true);
            Ok(CollectionReply::Id(id))
        }
        CollectionOp::AddMembers { collection, ids: members } => {
            catalog.collection_add(collection, &ids(&members)).map_err(err)?;
            doc.request_refresh(false);
            Ok(CollectionReply::Done)
        }
        CollectionOp::RemoveSelected => {
            if let Some(id) = doc.query.collection {
                catalog.remove_collection(id).map_err(err)?;
                doc.query.collection = None;
                doc.request_refresh(false);
            }
            Ok(CollectionReply::Done)
        }
        CollectionOp::Edit { name, rules } => {
            let Some(id) = doc.query.collection else { return Ok(CollectionReply::Done) };
            if !catalog.collections().map_err(err)?.iter().any(|c| c["id"] == id) {
                return Err("collection not found".into());
            }
            let text = rules.as_ref().map(|r| dumps(r, true, false, false));
            write(&doc.folder, |db| {
                match &text {
                    Some(text) => {
                        db.execute("UPDATE collections SET name=?,rules=? WHERE id=?", params![name, text, id])
                    }
                    None => db.execute("UPDATE collections SET name=? WHERE id=?", params![name, id]),
                }
                .map(|_| ())
            })?;
            doc.request_refresh(true);
            Ok(CollectionReply::Done)
        }
        CollectionOp::RemoveMembers { ids: members } => {
            let Some(id) = doc.query.collection else { return Ok(CollectionReply::Done) };
            write(&doc.folder, |db| {
                for photo in ids(&members) {
                    db.execute(
                        "DELETE FROM collection_members WHERE collection_id=? AND photo_id=?",
                        params![id, photo],
                    )?;
                }
                Ok(())
            })?;
            doc.request_refresh(true);
            Ok(CollectionReply::Done)
        }
        CollectionOp::Info { collection } => {
            let rows = catalog.collections().map_err(err)?;
            let row = rows.iter().find(|c| c["id"] == collection).ok_or("collection not found")?;
            let rules = match row["rules"].as_str() {
                Some(text) => Some(serde_json::from_str(text).map_err(err)?),
                None => None,
            };
            Ok(CollectionReply::Info { name: row["name"].as_str().unwrap_or("").to_owned(), rules })
        }
        CollectionOp::Containing { photo } => {
            let mut found = Vec::new();
            for row in catalog.collections().map_err(err)? {
                let collection = collection_of(&row);
                if catalog.collection_ids(collection.id).map_err(err)?.contains(&(photo as i64)) {
                    found.push(collection);
                }
            }
            Ok(CollectionReply::Collections(found))
        }
        CollectionOp::List => {
            Ok(CollectionReply::Collections(catalog.collections().map_err(err)?.iter().map(collection_of).collect()))
        }
        CollectionOp::ToggleQuick { ids: members, label } => {
            let (id, added) = workflow::toggle_quick(catalog, &ids(&members), &label).map_err(err)?;
            doc.request_refresh(false);
            Ok(CollectionReply::Toggled { id, added })
        }
        CollectionOp::Quick { label } => {
            Ok(CollectionReply::Id(workflow::quick_collection(catalog, &label).map_err(err)?))
        }
    }
}
