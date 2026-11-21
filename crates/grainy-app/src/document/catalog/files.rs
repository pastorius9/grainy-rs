//! File management on the catalogue (work package 22: luma/manager.py `rename`, `move`, `move_selected_folder`,
//! `remove(True)`, `relink`, `backup`, `restore`, `reload_current`; luma/folder_panel.py `apply_many`), done with
//! grainy-library's ports of the oracle's own functions (`library.move_files`, `move_folder`, `relink`,
//! `restore_backup`, `folder_locations.apply_relinks`) on the document's catalogue.
//!
//! What the window does around them is the oracle's too: the open photo's pending change is stored first
//! (`w.commit()`), the open photo is left while its path changes and opened again afterwards (`reload_current`,
//! `apply_many`), and the list is read again. Original files are only ever moved by `move_files` / `move_folder`
//! (which refuse existing targets and move back on failure); nothing here deletes a file.

use std::path::{Path, PathBuf};

use grainy_library::folder_locations::Plan;
use serde_json::{json, Value};

use super::{err, local_now, unique_suffix, CatalogDocument};
use crate::document::Document;

/// One operation.
#[derive(Debug, Clone, PartialEq)]
pub enum FileOp {
    /// `w.commit()` (and `save_keywords`): the open photo's pending change is stored.
    Commit,
    /// `manager.rename` / `manager.move` after their boxes: `library.move_files(catalog, mapping)` then
    /// `reload_current`. Answers [`FileReply::Count`] (the moved files).
    MoveFiles { mapping: Vec<(String, String)> },
    /// `manager.move_selected_folder`: `library.move_folder(catalog, source, destination)` then `reload_current`.
    MoveFolder { source: String, destination: String },
    /// `manager.remove(True)` after the files went to the recycle bin: every row (virtual copies too) whose path is
    /// one of `paths` leaves the catalogue. Answers [`FileReply::Count`].
    RemovePaths { paths: Vec<String> },
    /// Refresh the listing after originals and their virtual copies have been recycled.
    Trashed { ids: Vec<i64> },
    /// `manager.relink`: `library.relink(catalog, id, path)` then `reload_current`.
    Relink { id: u64, path: String },
    /// `manager.backup`: `catalog.backup(path)` after the commit.
    Backup { path: PathBuf },
    /// `manager.restore` after its question: `library.restore_backup(catalog, path)`; the collection, the rules and
    /// the folder of the list are cleared and every thumbnail is made again.
    Restore { path: PathBuf },
    /// `FolderPanel.apply_many`: a backup `backups/folder-location-….sqlite`, then
    /// `folder_locations.apply_relinks(catalog, plans)`; the open photo is left and opened again when it moved.
    /// Answers [`FileReply::Count`].
    ApplyRelinks { plans: Vec<Plan> },
    /// `photo_model.invalidate_thumbnails(ids)`: the thumbnails of `ids` are read again (a rebuilt preview).
    Invalidate { ids: Vec<u64> },
}

/// What an operation answers.
#[derive(Debug, Clone, PartialEq)]
pub enum FileReply {
    Done,
    Count(usize),
}

/// Runs `op` on the catalogue document (called by `Document::file_op`).
pub(super) fn apply(doc: &mut CatalogDocument, op: FileOp) -> Result<FileReply, String> {
    if doc.read_only {
        return Err("read-only".into());
    }
    if let FileOp::Invalidate { ids } = &op {
        for id in ids {
            doc.bump(*id);
        }
        return Ok(FileReply::Done);
    }
    if let FileOp::Trashed { ids } = &op {
        // The command flushed pending edits before it began recycling files.
        forget_removed(doc, ids);
        return Ok(FileReply::Count(ids.len()));
    }
    commit(doc)?;
    match op {
        FileOp::Invalidate { .. } | FileOp::Trashed { .. } => Ok(FileReply::Done),
        FileOp::Commit => Ok(FileReply::Done),
        FileOp::MoveFiles { mapping } => {
            for (src, dst) in &mapping {
                for path in [src, dst] {
                    crate::paths::writable(Path::new(path)).map_err(err)?;
                }
            }
            let count = grainy_library::library::move_files(&doc.catalog, &mapping).map_err(err)?;
            reload_current(doc);
            Ok(FileReply::Count(count))
        }
        FileOp::MoveFolder { source, destination } => {
            for path in [&source, &destination] {
                crate::paths::writable(Path::new(path)).map_err(err)?;
            }
            if grainy_platform::user_paths::contains_oracle(
                Path::new(&source),
                &grainy_platform::Environment::current(),
            ) || grainy_platform::user_paths::resolved_within(doc.catalog.directory(), Path::new(&source))
            {
                return Err("cannot move a folder containing application data".into());
            }
            grainy_library::library::move_folder(&doc.catalog, &source, &destination).map_err(err)?;
            reload_current(doc);
            Ok(FileReply::Done)
        }
        FileOp::RemovePaths { paths } => {
            let ids: Vec<i64> = doc
                .catalog
                .photos()
                .map_err(err)?
                .iter()
                .filter(|p| p["path"].as_str().is_some_and(|path| paths.iter().any(|q| q == path)))
                .filter_map(|p| p["id"].as_i64())
                .collect();
            doc.catalog.remove_photos(&ids).map_err(err)?;
            forget_removed(doc, &ids);
            Ok(FileReply::Count(ids.len()))
        }
        FileOp::Relink { id, path } => {
            grainy_library::library::relink(&doc.catalog, id as i64, &path).map_err(err)?;
            reload_current(doc);
            Ok(FileReply::Done)
        }
        FileOp::Backup { path } => {
            crate::paths::writable(&path).map_err(err)?;
            doc.catalog.backup(&path).map_err(err)?;
            Ok(FileReply::Done)
        }
        FileOp::Restore { path } => restore(doc, &path),
        FileOp::ApplyRelinks { plans } => apply_relinks(doc, &plans),
    }
}

fn forget_removed(doc: &mut CatalogDocument, ids: &[i64]) {
    let removed: std::collections::HashSet<i64> = ids.iter().copied().collect();
    if doc.open.as_ref().is_some_and(|open| removed.contains(&(open.id as i64))) {
        doc.clear_active_photo();
    }
    doc.listing.photos.retain(|photo| !removed.contains(&(photo.id as i64)));
    doc.request_refresh(true);
}

/// `w.commit()`: the pending change of the open photo is stored.
fn commit(doc: &mut CatalogDocument) -> Result<(), String> {
    doc.edit.commit("보정 변경");
    Document::flush(doc)
}

/// Port of `manager.reload_current`: the open photo is left, the list is read again, and the photo opens again when
/// it is still in the catalogue. Every listed photo's thumbnail is asked for again (paths may have changed).
fn reload_current(doc: &mut CatalogDocument) {
    let id = doc.open.as_ref().map(|o| o.id);
    doc.clear_active_photo();
    let ids: Vec<u64> = doc.listing.photos.iter().map(|p| p.id).collect();
    for id in ids {
        doc.bump(id);
    }
    if let Some(id) = id.filter(|id| doc.catalog.photo(*id as i64).ok().flatten().is_some()) {
        doc.restore = Some(id);
    }
    doc.request_refresh(false);
}

/// Port of `manager.restore` after the question (`restore_backup`, then the list without collection, rules and
/// folder; `photo_model.invalidate_thumbnails()`).
fn restore(doc: &mut CatalogDocument, path: &Path) -> Result<FileReply, String> {
    // SQLite may create a WAL index even on a read-only connection to a live backup.
    crate::paths::writable(path).map_err(err)?;
    crate::paths::writable(&doc.catalog.directory().join("backups")).map_err(err)?;
    grainy_library::library::restore_backup(&doc.catalog, path).map_err(err)?;
    doc.clear_active_photo();
    doc.query.collection = None;
    doc.query.rules = json!({});
    doc.query.folder = None;
    doc.memory.clear();
    doc.overlay.clear();
    let ids: Vec<u64> = doc.listing.photos.iter().map(|p| p.id).chain(doc.versions.keys().copied()).collect();
    for id in ids {
        doc.bump(id);
    }
    doc.listing.photos.clear();
    doc.request_refresh(true);
    Ok(FileReply::Done)
}

/// Port of `FolderPanel.apply_many` (the catalogue part; the window's folder filter and expanded folders are
/// rebased by the caller).
fn apply_relinks(doc: &mut CatalogDocument, plans: &[Plan]) -> Result<FileReply, String> {
    let stamp = local_now().format("%Y%m%d-%H%M%S");
    let backup = doc.folder.join("backups").join(format!("folder-location-{stamp}-{}.sqlite", unique_suffix()));
    doc.catalog.backup(&backup).map_err(err)?;
    let current = doc.open.as_ref().map(|o| o.id);
    let affected: Vec<u64> = plans.iter().flat_map(|p| p.updates.iter().map(|(_, id)| *id as u64)).collect();
    let reopen = current.filter(|id| affected.contains(id));
    if reopen.is_some() {
        doc.clear_active_photo();
    }
    let result = grainy_library::folder_locations::apply_relinks(&doc.catalog, plans).map_err(err);
    if let Some(id) = reopen {
        doc.restore = Some(id);
    }
    let count = result?;
    for id in affected {
        doc.bump(id);
    }
    doc.request_refresh(false);
    Ok(FileReply::Count(count))
}

/// The paths of the photos `ids` (as the catalogue stores them), each once, in the order of `ids`
/// (`dict.fromkeys(p['path'] for p in selected_photos())`).
pub fn paths_of(catalog: &grainy_catalog::Catalog, ids: &[u64]) -> Result<Vec<String>, String> {
    let mut statement = catalog
        .connection()
        .prepare_cached(
            "SELECT p.path FROM json_each(?1) AS selection
         JOIN photos AS p ON p.id = selection.value
         GROUP BY p.path COLLATE BINARY ORDER BY MIN(selection.key)",
        )
        .map_err(err)?;
    let selected = serde_json::to_string(ids).map_err(err)?;
    let paths = statement.query_map([selected], |row| row.get(0)).map_err(err)?;
    paths.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
}

/// The rows `Catalog.photos()` gives (for the duplicate search and the catalogue check).
pub fn photo_rows(catalog: &grainy_catalog::Catalog) -> Result<Vec<Value>, String> {
    catalog.photos().map_err(err)
}
