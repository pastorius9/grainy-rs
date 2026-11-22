//! A library folder on disk: the oracle's SQLite catalogue (`grainy-catalog`), listed through
//! `grainy_catalog::query` (filters, search, folders, collections, sort, stacks), with the oracle's commit and undo
//! semantics (luma/app.py `activate`, `commit`, `save_undo`, `undo`, `redo`, `apply_restored`, `restore_history`,
//! `paste_edits` with `manager.edit_photo`, `set_rating`, `set_flag`, `manager.set_label`, `manager.remove`) and the
//! import of `import_paths` / `import_next` (add in place).
//!
//! A library opened read-only is read and listed from a private snapshot in the Rust app's data folder: nothing of the original folder is ever written, and edits,
//! ratings and labels stay in memory.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use chrono::NaiveDateTime;
use grainy_catalog::folders::Platform;
use grainy_catalog::rusqlite::{self, types::ValueRef};
use grainy_catalog::{query, Catalog};
use grainy_core::{normalized, Settings};
use serde_json::{json, Map, Value};

use super::{
    Collection, Document, FolderInputs, FolderNode, HistoryEntry, Kind, LabelField, Listing, ListingJob, PhotoEntry,
    Query, ThumbCache,
};
use crate::state::{Change, Commit, EditState};

pub mod collections;
pub mod files;
pub mod stacks;

/// `adobe_history.HISTORY_PREFIX`: history rows imported from Lightroom keep their own label.
const HISTORY_PREFIX: &str = "Lightroom 상태 · ";

/// The clock of smart rules ("last 7 days"): the local time.
pub type Clock = Arc<dyn Fn() -> NaiveDateTime + Send + Sync>;

fn local_now() -> NaiveDateTime {
    chrono::Local::now().naive_local()
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Settings as the oracle stores them (`engine.normalized`, keys in the defaults' order).
fn stored(settings: &Settings) -> Value {
    grainy_catalog::settings(&Value::Object(settings.0.clone()))
        .map(|s| Value::Object(s.0))
        .unwrap_or_else(|_| Value::Object(settings.0.clone()))
}

fn settings_value(value: &Value) -> Settings {
    match value {
        Value::Object(map) => normalized(map),
        _ => normalized(&Map::new()),
    }
}

fn dim_u32(r: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<u32>> {
    Ok(match r.get_ref(index)? {
        ValueRef::Integer(v) if v >= 0 => u32::try_from(v).ok(),
        _ => None,
    })
}

fn photo_entry_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PhotoEntry> {
    let id: i64 = r.get(0)?;
    let source: Option<i64> = r.get(6)?;
    let width = dim_u32(r, 8)?;
    let height = dim_u32(r, 9)?;
    Ok(PhotoEntry {
        id: id as u64,
        path: PathBuf::from(r.get::<_, String>(1)?),
        name: r.get(2)?,
        rating: r.get(3)?,
        flag: r.get(4)?,
        label: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
        virtual_source: source.filter(|s| *s != 0).map(|s| s as u64),
        stack: r.get(7)?,
        original: width.zip(height),
    })
}

/// The photo rows of `ids` on an open catalogue connection, in the order of `ids`.
/// Extracts `width`/`height` inside SQLite (`json_extract`) instead of deserializing the full EXIF JSON
/// blob per photo, and uses a single sequential table scan for large unfiltered views.
fn photo_rows_on(connection: &rusqlite::Connection, ids: &[i64], total: usize) -> Result<Vec<PhotoEntry>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    const COLS: &str = "id,path,name,rating,flag,label,virtual_source,stack_id,\
        CASE WHEN metadata!='{}' AND json_valid(metadata) THEN json_extract(metadata,'$.width') END,\
        CASE WHEN metadata!='{}' AND json_valid(metadata) THEN json_extract(metadata,'$.height') END";
    // Large listing (e.g. All Photos / big folder): one sequential table scan beats hundreds of IN(...) B-tree probes.
    if ids.len() > 1000 && ids.len().saturating_mul(4) >= total {
        let mut positions: HashMap<i64, usize> = HashMap::with_capacity(ids.len());
        for (pos, &id) in ids.iter().enumerate() {
            positions.entry(id).or_insert(pos);
        }
        let mut slots: Vec<Option<PhotoEntry>> = vec![None; ids.len()];
        let mut statement = connection.prepare_cached(&format!("SELECT {COLS} FROM photos")).map_err(err)?;
        let mapped = statement.query_map([], photo_entry_from_row).map_err(err)?;
        for row in mapped {
            let entry = row.map_err(err)?;
            if let Some(&pos) = positions.get(&(entry.id as i64)) {
                slots[pos] = Some(entry);
            }
        }
        return Ok(slots.into_iter().flatten().collect());
    }
    let mut rows: HashMap<i64, PhotoEntry> = HashMap::with_capacity(ids.len());
    for part in ids.chunks(900) {
        let marks = vec!["?"; part.len()].join(",");
        let sql = format!("SELECT {COLS} FROM photos WHERE id IN ({marks})");
        let mut statement = connection.prepare_cached(&sql).map_err(err)?;
        let mapped = statement.query_map(rusqlite::params_from_iter(part), photo_entry_from_row).map_err(err)?;
        for row in mapped {
            let entry = row.map_err(err)?;
            rows.insert(entry.id as i64, entry);
        }
    }
    Ok(ids.iter().filter_map(|id| rows.remove(id)).collect())
}

fn collections_of(connection: &rusqlite::Connection) -> Result<Vec<Collection>, String> {
    let mut statement =
        connection.prepare_cached("SELECT id,name,parent_id,rules FROM collections ORDER BY name").map_err(err)?;
    let rows = statement
        .query_map([], |r| {
            Ok(Collection {
                id: r.get(0)?,
                name: r.get(1)?,
                parent: r.get(2)?,
                smart: r.get::<_, Option<String>>(3)?.is_some(),
            })
        })
        .map_err(err)?;
    rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
}

/// `Catalog.removed_folders` read from the catalogue connection (the listing job has no `Catalog`).
fn removed_folders_of(connection: &rusqlite::Connection) -> Vec<String> {
    let text: Option<String> =
        connection.query_row("SELECT value FROM preferences WHERE key='removed_folders'", [], |r| r.get(0)).ok();
    text.and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect()))
        .unwrap_or_default()
}

/// The flat `folder_nodes` list as a tree in O(F) time.
fn folder_tree(nodes: &[Value]) -> Vec<FolderNode> {
    let mut by_parent: HashMap<Option<&str>, Vec<&Value>> = HashMap::with_capacity(nodes.len());
    for n in nodes {
        by_parent.entry(n["parent"].as_str()).or_default().push(n);
    }
    fn build(by_parent: &HashMap<Option<&str>, Vec<&Value>>, parent: Option<&str>) -> Vec<FolderNode> {
        let Some(children) = by_parent.get(&parent) else {
            return Vec::new();
        };
        children
            .iter()
            .map(|n| {
                let key = n["key"].as_str().unwrap_or("");
                FolderNode {
                    path: n["path"].as_str().unwrap_or("").to_owned(),
                    name: n["name"].as_str().unwrap_or("").to_owned(),
                    direct: n["direct"].as_i64().unwrap_or(0),
                    total: n["total"].as_i64().unwrap_or(0),
                    children: build(by_parent, Some(key)),
                }
            })
            .collect()
    }
    build(&by_parent, None)
}

/// Port of `refresh_lists`' work: `library_query.query`, the rows of the result, the folder tree and the
/// collections, executed on a single tuned SQLite connection.
pub fn read_listing(db: &Path, options: &Value, now: NaiveDateTime, generation: u64) -> Result<Listing, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let connection = query::open_connection(db).map_err(err)?;
    let result = query::query_listing_on(Platform::native(), &connection, options, &cancel, now).map_err(err)?;
    let total = result.total.max(0) as usize;
    let photos = photo_rows_on(&connection, &result.ids, total)?;
    let nodes = Platform::native().folder_nodes(&result.folders, &result.roots).map_err(err)?;
    Ok(Listing {
        generation,
        photos,
        total,
        folders: folder_tree(&nodes),
        collections: collections_of(&connection)?,
        folder_inputs: FolderInputs {
            rows: result.folders,
            roots: result.roots,
            removed: removed_folders_of(&connection),
        },
    })
}

/// The labels changed in memory in a read-only library.
#[derive(Debug, Clone, Default)]
struct Overlay {
    rating: Option<i64>,
    flag: Option<i64>,
    label: Option<String>,
}

/// A library folder.
pub struct CatalogDocument {
    catalog: Catalog,
    folder: PathBuf,
    /// The catalogue file listings read: the library's own, or the snapshot of a read-only library.
    listing_db: PathBuf,
    snapshot: Option<tempfile::TempDir>,
    read_only: bool,
    listing: Listing,
    query: Query,
    clock: Clock,
    /// The newest refresh asked for and the one shown.
    requested: u64,
    applied: u64,
    pending: bool,
    /// Open the first photo when the open one is not listed (`ensure_current`).
    ensure_current: bool,
    /// Safe mode: while no photo is open and no refresh asks for one, the first photo does not open by itself.
    hold_first: bool,
    /// The photo the saved session had open: opened when a listing shows it (`restore_photo_id`).
    restore: Option<u64>,
    open: Option<PhotoEntry>,
    edit: EditState,
    selection: BTreeSet<u64>,
    selection_anchor: Option<u64>,
    copied: Option<Settings>,
    versions: HashMap<u64, u64>,
    /// Read-only libraries: the edits of photos that are not open, in memory.
    memory: HashMap<u64, EditState>,
    overlay: HashMap<u64, Overlay>,
    history: Vec<HistoryEntry>,
    /// The open photo's commits not yet handed to Auto Sync.
    commits: Vec<(u64, Commit)>,
    prune_thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for CatalogDocument {
    fn drop(&mut self) {
        if let Some(handle) = self.prune_thread.take() {
            let _ = handle.join();
        }
    }
}

impl CatalogDocument {
    /// The writer of [`Document::edit_photos_batch`] (`retarget`: the values are fitted to each photo's file) and
    /// [`Document::edit_photos_batch_own`].
    fn write_batch(
        &mut self,
        ids: &[u64],
        settings: &[Settings],
        label: &str,
        retarget: bool,
    ) -> Result<Vec<u64>, String> {
        if ids.len() != settings.len() {
            return Err("일괄 보정 대상이 올바르지 않습니다.".into());
        }
        // `records()`: the pending change is committed and stored first.
        self.edit.commit("보정 변경");
        Document::flush(self)?;
        let mut values = Vec::new();
        for (&id, value) in ids.iter().zip(settings) {
            values.push(if retarget {
                crate::commands::sync::fitted(value, &Document::photo_info(self, id))?
            } else {
                value.clone()
            });
        }
        let open = self.open.as_ref().map(|o| o.id);
        if self.read_only {
            let mut changed = Vec::new();
            for (&id, value) in ids.iter().zip(&values) {
                if crate::state::settings_eq(&Document::stored_settings_of(self, id)?, value) {
                    continue;
                }
                if Some(id) == open {
                    self.edit.replace(value.clone(), label);
                    self.edit.take_journal();
                    self.edit.take_commits();
                    self.bump(id);
                } else {
                    self.edit_photo(id, value, label)?;
                }
                changed.push(id);
            }
            return Ok(changed);
        }
        let mut records = Vec::new();
        for &id in ids {
            let row = self.catalog.photo(id as i64).map_err(err)?.ok_or("photo not found")?;
            records.push(json!({"id": id, "path": row["path"], "settings": row["settings"]}));
        }
        let values: Vec<Value> = values.iter().map(stored).collect();
        let options = grainy_service::batch::ApplyOptions {
            label: label.to_owned(),
            remember: false,
            switch: false,
            ..Default::default()
        };
        let changed = grainy_service::batch::apply(&self.catalog, &records, &values, &options, None).map_err(err)?;
        let changed: Vec<u64> = changed.into_iter().map(|id| id as u64).collect();
        for &id in &changed {
            self.bump(id);
        }
        // `refresh_command_edits`: the open photo's state and stacks are read again from the catalogue.
        if let Some(id) = open.filter(|id| changed.contains(id)) {
            let version = self.edit.version();
            let mut state = self.load(id)?;
            // The picture is developed again: the version only ever grows.
            let live = state.settings().clone();
            while state.version() <= version {
                state.set_all(live.clone());
            }
            self.edit = state;
            self.refresh_history();
        }
        Ok(changed)
    }

    /// Opens (or creates) the library in `folder` for writing. Refuses the Python Grainy's folders (they open
    /// read-only, through [`CatalogDocument::open_read_only`]).
    pub fn open(folder: &Path) -> Result<Self, String> {
        if crate::paths::is_oracle_folder(folder, &crate::paths::Locations::current()) {
            return Err(format!("{} belongs to the Python Grainy: it opens read-only only", folder.display()));
        }
        for name in [
            "catalog.sqlite",
            "catalog.sqlite-wal",
            "catalog.sqlite-shm",
            "catalog.sqlite-journal",
            "backups",
            "thumbnails",
            "previews",
        ] {
            crate::paths::writable(&folder.join(name)).map_err(err)?;
        }
        std::fs::create_dir_all(folder).map_err(err)?;
        let mut doc = Self::with_catalog(Catalog::new(folder).map_err(err)?)?;
        let has_cache_files = ["thumbnails", "previews"].iter().any(|sub| {
            std::fs::read_dir(folder.join(sub)).ok().is_some_and(|mut it| it.next().is_some())
        });
        if has_cache_files && !crate::context::FROZEN_TIMING.load(std::sync::atomic::Ordering::Relaxed) {
            let dir = folder.to_path_buf();
            doc.prune_thread = std::thread::Builder::new()
                .name("grainy-cache-prune".into())
                .spawn(move || {
                    if let Ok(stats) =
                        grainy_catalog::preview::prune_cache(&dir, grainy_catalog::preview::DEFAULT_CACHE_LIMIT_BYTES)
                    {
                        if stats.removed_files > 0 {
                            log::info!(
                                "pruned {} stale/LRU cache files ({:.1} MiB freed)",
                                stats.removed_files,
                                stats.freed_bytes as f64 / (1024.0 * 1024.0)
                            );
                        }
                    }
                })
                .ok();
        }
        Ok(doc)
    }

    /// The document over an already opened catalogue (tests give one with deterministic clocks).
    pub fn with_catalog(catalog: Catalog) -> Result<Self, String> {
        let folder = catalog.directory().to_path_buf();
        let read_only = catalog.is_reader();
        let listing_db = folder.join("catalog.sqlite");
        Ok(Self::build(catalog, folder, listing_db, read_only))
    }

    /// Opens a verified private copy of the catalogue and its committed WAL in `scratch`.
    pub fn open_read_only(folder: &Path, scratch: &Path) -> Result<Self, String> {
        // Validate before creating a directory or removing an old snapshot. In particular,
        // scratch == folder must never unlink the source catalogue and its WAL.
        if crate::paths::is_oracle_folder(scratch, &crate::paths::Locations::current())
            || crate::paths::is_within(scratch, folder)
        {
            return Err("the snapshot folder must be the Rust app's own".into());
        }
        // SQLite may create a shared-memory index even for a read-only live WAL connection.
        // Only open SQLite on our private copy. Reject a changing source instead of mixing generations.
        std::fs::create_dir_all(scratch).map_err(err)?;
        let snapshot = tempfile::tempdir_in(scratch).map_err(err)?;
        let db = "catalog.sqlite";
        let wal = "catalog.sqlite-wal";
        let journal = folder.join("catalog.sqlite-journal");
        if journal.try_exists().map_err(err)? {
            return Err("the source library has a rollback journal; close it before opening a snapshot".into());
        }
        let fingerprint = |name: &str| -> std::io::Result<Option<[u8; 32]>> {
            match grainy_library::fsops::digest_bytes(&folder.join(name)) {
                Ok(hash) => Ok(Some(hash)),
                Err(e) if name == wal && e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e),
            }
        };
        let before = [fingerprint(db).map_err(err)?, fingerprint(wal).map_err(err)?];
        for (name, hash) in [db, wal].into_iter().zip(before) {
            if let Some(expected) = hash {
                let target = snapshot.path().join(name);
                let mut input = std::fs::File::open(folder.join(name)).map_err(err)?;
                std::io::copy(&mut input, &mut std::fs::File::create(&target).map_err(err)?).map_err(err)?;
                if grainy_library::fsops::digest_bytes(&target).map_err(err)? != expected {
                    return Err("the source library changed while taking the snapshot; try again".into());
                }
            }
        }
        if journal.try_exists().map_err(err)?
            || before != [fingerprint(db).map_err(err)?, fingerprint(wal).map_err(err)?]
        {
            return Err("the source library changed while taking the snapshot; try again".into());
        }
        let listing = snapshot.path().join(db);
        // Recover committed WAL frames in the copy and leave a self-contained read-only database.
        let copied = rusqlite::Connection::open(&listing).map_err(err)?;
        copied.execute_batch("PRAGMA journal_mode=DELETE").map_err(err)?;
        drop(copied);
        let catalog = Catalog::open_reader(snapshot.path()).map_err(err)?;
        let mut doc = Self::build(catalog, folder.to_owned(), listing, true);
        doc.snapshot = Some(snapshot);
        Ok(doc)
    }

    fn build(catalog: Catalog, folder: PathBuf, listing_db: PathBuf, read_only: bool) -> Self {
        Self {
            catalog,
            folder,
            listing_db,
            snapshot: None,
            read_only,
            listing: Listing::default(),
            query: Query::default(),
            clock: Arc::new(local_now),
            requested: 1,
            applied: 0,
            pending: true,
            ensure_current: true,
            hold_first: false,
            restore: None,
            open: None,
            edit: EditState::default(),
            selection: BTreeSet::new(),
            selection_anchor: None,
            copied: None,
            versions: HashMap::new(),
            memory: HashMap::new(),
            overlay: HashMap::new(),
            history: Vec::new(),
            commits: Vec::new(),
            prune_thread: None,
        }
    }

    /// Sets the clock of smart rules (tests).
    pub fn set_clock(&mut self, clock: Clock) {
        self.clock = clock;
    }

    /// The catalogue (read-only through `&`; writes go through the document).
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    /// The library folder.
    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// The listing job of the newest refresh, once (the window runs it in the background).
    pub fn listing_job(&mut self) -> Option<ListingJob> {
        if !self.pending {
            return None;
        }
        self.pending = false;
        let stack_orders = self.catalog.preference("stack_orders", json!({})).unwrap_or_else(|_| json!({}));
        let options = self.query.options(stack_orders);
        let db = self.listing_db.clone();
        let clock = self.clock.clone();
        let generation = self.requested;
        Some(Box::new(move || read_listing(&db, &options, clock(), generation)))
    }

    /// Whether a refresh was asked for and not answered.
    pub fn listing_pending(&self) -> bool {
        self.applied < self.requested
    }

    /// Asks for the list to be read again.
    pub fn request_refresh(&mut self, ensure_current: bool) {
        self.requested += 1;
        self.pending = true;
        self.ensure_current |= ensure_current;
    }

    /// Shows a listing (stale ones are dropped). The open photo stays open when it is listed; otherwise the first
    /// photo opens when `ensure_current` was asked for (`refresh_lists(ensure_current=True)`).
    pub fn apply_listing(&mut self, mut listing: Listing) {
        if listing.generation < self.applied || listing.generation > self.requested {
            return;
        }
        self.applied = listing.generation;
        for photo in &mut listing.photos {
            if let Some(o) = self.overlay.get(&photo.id) {
                photo.rating = o.rating.unwrap_or(photo.rating);
                photo.flag = o.flag.unwrap_or(photo.flag);
                if let Some(label) = &o.label {
                    photo.label = label.clone();
                }
            }
        }
        self.listing = listing;
        if !self.selection.is_empty() {
            let mut present: HashSet<u64> = HashSet::with_capacity(self.selection.len());
            for photo in &self.listing.photos {
                if self.selection.contains(&photo.id) {
                    present.insert(photo.id);
                    if present.len() == self.selection.len() {
                        break;
                    }
                }
            }
            self.selection.retain(|id| present.contains(id));
        }
        let ensure = std::mem::take(&mut self.ensure_current);
        if ensure || self.open.is_some() {
            self.hold_first = false;
        }
        // The saved session's photo comes first when it is listed (`desired in positions`).
        if let Some(index) = self.restore.take().and_then(|id| self.listing.photos.iter().position(|p| p.id == id)) {
            self.select(index);
            return;
        }
        let open_entry =
            self.open.as_ref().and_then(|open| self.listing.photos.iter().find(|p| p.id == open.id).cloned());
        match (self.open.as_ref().map(|p| p.id), open_entry) {
            (Some(id), Some(mut entry)) => {
                let original = self.open.as_ref().and_then(|o| o.original);
                entry.original = entry.original.or(original);
                self.open = Some(entry);
                self.selection.insert(id);
            }
            (Some(_), None) if !ensure => {}
            (None, _) if self.hold_first => {}
            _ => {
                if self.listing.photos.is_empty() {
                    if ensure || self.open.is_some() {
                        self.clear_active_photo();
                    }
                } else {
                    self.select(0);
                }
            }
        }
    }

    /// Reads the newest listing now (tests and the first frame of a small library).
    pub fn refresh_now(&mut self) -> Result<(), String> {
        self.pending = true;
        if let Some(job) = self.listing_job() {
            let listing = job()?;
            self.apply_listing(listing);
        }
        Ok(())
    }

    /// Port of `clear_active_photo`.
    fn clear_active_photo(&mut self) {
        if !self.leave_current() {
            return;
        }
        self.open = None;
        self.edit = EditState::default();
        self.history.clear();
    }

    /// Commits and stores the open photo's pending change before it is left; a read-only library keeps its
    /// state in memory.
    fn leave_current(&mut self) -> bool {
        self.edit.commit("보정 변경");
        if let Err(message) = Document::flush(self) {
            log::warn!("cannot store the edit: {message}");
            return false;
        }
        if let (true, Some(open)) = (self.read_only, &self.open) {
            self.memory.insert(open.id, std::mem::take(&mut self.edit));
        }
        true
    }

    /// Port of `activate`'s reads: the stored settings and the `undo:{id}` stacks.
    fn load(&mut self, id: u64) -> Result<EditState, String> {
        if let Some(edit) = self.memory.remove(&id) {
            return Ok(edit);
        }
        let row = self.catalog.photo(id as i64).map_err(err)?.ok_or("photo not found")?;
        let settings = settings_value(&row["settings"]);
        let state = self.catalog.preference(&format!("undo:{id}"), json!({})).map_err(err)?;
        let stack = |key: &str| -> Vec<Settings> {
            state.get(key).and_then(Value::as_array).map(|a| a.iter().map(settings_value).collect()).unwrap_or_default()
        };
        Ok(EditState::with_stacks(settings, stack("undo"), stack("redo")))
    }

    fn refresh_history(&mut self) {
        self.history.clear();
        let Some(open) = &self.open else { return };
        match self.catalog.histories(open.id as i64) {
            Ok(rows) => {
                for row in rows {
                    let label = row["label"].as_str().unwrap_or("").to_owned();
                    let shown = if label.starts_with(HISTORY_PREFIX) { label } else { format!("{label} 이전") };
                    self.history.push(HistoryEntry {
                        label: shown,
                        created: row["created"].as_str().unwrap_or("").to_owned(),
                        settings: Some(settings_value(&row["settings"])),
                    });
                }
            }
            Err(error) => log::warn!("cannot read the history: {error}"),
        }
    }

    fn bump(&mut self, id: u64) {
        *self.versions.entry(id).or_insert(0) += 1;
    }

    /// Port of `save_undo`.
    fn save_undo(&self, id: u64) -> grainy_catalog::Result<()> {
        let (undo, redo) = self.edit.stored_stacks();
        let value = json!({
            "undo": undo.iter().map(stored).collect::<Vec<_>>(),
            "redo": redo.iter().map(stored).collect::<Vec<_>>(),
        });
        self.catalog.save_preference(&format!("undo:{id}"), &value)
    }

    /// Port of `manager.edit_photo` for a photo that is not open: one stored step with its own undo stack.
    fn edit_photo(&mut self, id: u64, settings: &Settings, label: &str) -> Result<(), String> {
        if self.read_only {
            let mut edit = match self.memory.remove(&id) {
                Some(edit) => edit,
                None => self.load(id)?,
            };
            edit.replace(settings.clone(), label);
            edit.take_journal();
            self.memory.insert(id, edit);
            self.bump(id);
            return Ok(());
        }
        let row = self.catalog.photo(id as i64).map_err(err)?.ok_or("photo not found")?;
        let previous = row["settings"].clone();
        let new = stored(settings);
        if grainy_catalog::json::equivalent(&previous, &new) {
            return Ok(());
        }
        let mut state = self.catalog.preference(&format!("undo:{id}"), json!({"undo": [], "redo": []})).map_err(err)?;
        let mut undo = state.get("undo").and_then(Value::as_array).cloned().unwrap_or_default();
        undo.push(previous);
        let skip = undo.len().saturating_sub(crate::state::UNDO_LIMIT);
        state["undo"] = Value::Array(undo.split_off(skip));
        state["redo"] = json!([]);
        self.catalog
            .immediate(|| {
                self.catalog.edit(id as i64, settings, label)?;
                self.catalog.save_preference(&format!("undo:{id}"), &state)
            })
            .map_err(err)?;
        self.bump(id);
        Ok(())
    }

    fn selected_ids(&self) -> Vec<u64> {
        self.selected_indices().iter().map(|i| self.listing.photos[*i].id).collect()
    }

    /// The stored settings of a photo that is not open (memory first in a read-only library).
    fn stored_settings(&self, id: u64) -> Result<Settings, String> {
        if let Some(edit) = self.memory.get(&id) {
            return Ok(edit.settings().clone());
        }
        let row = self.catalog.photo(id as i64).map_err(err)?.ok_or("photo not found")?;
        Ok(settings_value(&row["settings"]))
    }

    /// Port of `manager.virtual_copy`: a virtual copy of each selected photo.
    pub fn virtual_copies(&mut self) -> Result<usize, String> {
        if self.read_only {
            return Err("read-only".into());
        }
        self.edit.commit("보정 변경");
        Document::flush(self)?;
        let ids = self.selected_ids();
        for id in &ids {
            self.catalog.virtual_copy(*id as i64).map_err(err)?;
        }
        self.request_refresh(false);
        Ok(ids.len())
    }
}

impl Document for CatalogDocument {
    fn kind(&self) -> Kind {
        Kind::Catalog { folder: self.folder.clone(), read_only: self.read_only }
    }

    fn photos(&self) -> &[PhotoEntry] {
        &self.listing.photos
    }

    fn total(&self) -> usize {
        self.listing.total
    }

    fn current_index(&self) -> Option<usize> {
        let id = self.open.as_ref()?.id;
        self.listing.photos.iter().position(|p| p.id == id)
    }

    fn current(&self) -> Option<&PhotoEntry> {
        self.open.as_ref()
    }

    fn select(&mut self, index: usize) -> bool {
        let Some(entry) = self.listing.photos.get(index).cloned() else { return false };
        if self.open.as_ref().is_some_and(|o| o.id == entry.id) {
            self.selection = BTreeSet::from([entry.id]);
            self.selection_anchor = Some(entry.id);
            return false;
        }
        if !self.leave_current() {
            return false;
        }
        self.selection = BTreeSet::from([entry.id]);
        self.selection_anchor = Some(entry.id);
        match self.load(entry.id) {
            Ok(edit) => {
                self.edit = edit;
                self.open = Some(entry);
                self.refresh_history();
                true
            }
            Err(message) => {
                log::warn!("cannot open photo {}: {message}", entry.id);
                self.open = None;
                self.edit = EditState::default();
                self.history.clear();
                true
            }
        }
    }

    fn toggle_selected(&mut self, index: usize) {
        let Some(photo) = self.listing.photos.get(index) else { return };
        let id = photo.id;
        let current_id = self.current_id();
        if current_id != 0 {
            self.selection.insert(current_id);
        }
        if id == current_id {
            self.selection_anchor = Some(id);
            return;
        }
        if !self.selection.remove(&id) {
            self.selection.insert(id);
        }
        self.selection_anchor = Some(id);
    }

    fn select_range(&mut self, index: usize, additive: bool) {
        if index >= self.listing.photos.len() {
            return;
        }
        let anchor = self
            .selection_anchor
            .and_then(|id| self.listing.photos.iter().position(|p| p.id == id))
            .or_else(|| self.current_index())
            .unwrap_or(index);
        if self.selection_anchor.is_none() {
            self.selection_anchor = self.listing.photos.get(anchor).map(|p| p.id);
        }
        if !additive {
            self.selection.clear();
            let current_id = self.current_id();
            if current_id != 0 {
                self.selection.insert(current_id);
            }
        }
        let (lo, hi) = (anchor.min(index), anchor.max(index));
        for photo in &self.listing.photos[lo..=hi] {
            self.selection.insert(photo.id);
        }
    }

    fn select_all(&mut self) {
        self.selection = self.listing.photos.iter().map(|p| p.id).collect();
    }

    fn deselect_others(&mut self) {
        let current_id = self.current_id();
        self.selection = if current_id != 0 { BTreeSet::from([current_id]) } else { BTreeSet::new() };
        self.selection_anchor = (current_id != 0).then_some(current_id);
    }

    fn is_selected(&self, id: u64) -> bool {
        self.selection.contains(&id)
    }

    fn selected_indices(&self) -> Vec<usize> {
        let out: Vec<usize> = self
            .listing
            .photos
            .iter()
            .enumerate()
            .filter(|(_, p)| self.selection.contains(&p.id))
            .map(|(i, _)| i)
            .collect();
        if out.is_empty() {
            return self.current_index().into_iter().collect();
        }
        out
    }

    fn edit(&self) -> &EditState {
        &self.edit
    }

    fn edit_mut(&mut self) -> &mut EditState {
        &mut self.edit
    }

    fn settings_of(&self, index: usize) -> Settings {
        let Some(photo) = self.listing.photos.get(index) else { return normalized(&Map::new()) };
        if self.open.as_ref().is_some_and(|o| o.id == photo.id) {
            return self.edit.settings().clone();
        }
        self.stored_settings(photo.id).unwrap_or_else(|_| normalized(&Map::new()))
    }

    fn version_of(&self, index: usize) -> u64 {
        let Some(photo) = self.listing.photos.get(index) else { return 0 };
        let stored = self.versions.get(&photo.id).copied().unwrap_or(0);
        if self.open.as_ref().is_some_and(|o| o.id == photo.id) && self.read_only {
            // In-memory edits: the live state's version.
            return stored.wrapping_mul(1 << 20).wrapping_add(self.edit.version());
        }
        stored
    }

    fn thumb_cache(&self, index: usize) -> Option<ThumbCache> {
        let photo = self.listing.photos.get(index)?;
        // A read-only photo edited in memory is rendered from its live settings, not the stored ones.
        if self.read_only
            && (self.memory.contains_key(&photo.id)
                || self.open.as_ref().is_some_and(|o| o.id == photo.id && self.edit.depth() != (0, 0)))
        {
            return None;
        }
        Some(ThumbCache {
            snapshot: self.snapshot.as_ref().map(|s| s.path().to_owned()),
            directory: self.folder.clone(),
            id: photo.id as i64,
            writable: !self.read_only,
        })
    }

    fn set_original(&mut self, id: u64, size: (u32, u32)) {
        if let Some(open) = self.open.as_mut().filter(|o| o.id == id) {
            open.original = Some(size);
        }
        if let Some(photo) = self.listing.photos.iter_mut().find(|p| p.id == id) {
            photo.original = Some(size);
        }
    }

    fn flush(&mut self) -> Result<(), String> {
        let Some(id) = self.open.as_ref().map(|o| o.id) else { return Ok(()) };
        if !self.edit.has_journal() {
            return Ok(());
        }
        if !self.read_only {
            self.catalog
                .immediate(|| {
                    for change in self.edit.journal() {
                        match change {
                            Change::Committed { label, settings } => self.catalog.edit(id as i64, settings, label)?,
                            Change::Restored { settings } => self.catalog.set_settings(id as i64, settings)?,
                        }
                    }
                    self.save_undo(id)
                })
                .map_err(err)?;
            self.refresh_history();
        }
        self.edit.take_journal();
        self.commits.extend(self.edit.take_commits().into_iter().map(|c| (id, c)));
        self.bump(id);
        Ok(())
    }

    fn history(&self) -> Vec<HistoryEntry> {
        if self.read_only {
            // The stored history of the original, then nothing of the in-memory steps (they are not rows).
            return self.history.clone();
        }
        self.history.clone()
    }

    fn stored_settings_of(&self, id: u64) -> Result<Settings, String> {
        if self.open.as_ref().is_some_and(|o| o.id == id) {
            return Ok(self.edit.settings().clone());
        }
        self.stored_settings(id)
    }

    fn photo_info(&self, id: u64) -> Value {
        match self.catalog.photo(id as i64) {
            Ok(Some(row)) if row["info"].is_object() => row["info"].clone(),
            _ => {
                let format = self
                    .index_of(id)
                    .and_then(|i| self.listing.photos.get(i))
                    .and_then(|p| p.path.extension().map(|e| e.to_string_lossy().to_lowercase()));
                json!({ "format": format.unwrap_or_default() })
            }
        }
    }

    fn edit_photos(&mut self, ids: &[u64], settings: &[Settings], label: &str) -> Result<usize, String> {
        if ids.len() != settings.len() {
            return Err("일괄 보정 대상이 올바르지 않습니다.".into());
        }
        let current = self.open.as_ref().map(|o| o.id);
        let mut count = 0;
        for (&id, value) in ids.iter().zip(settings) {
            let value = crate::commands::sync::fitted(value, &Document::photo_info(self, id))?;
            if Some(id) == current {
                // The window's own state: one undo step, stored by the flush.
                self.edit.replace(value, label);
                Document::flush(self)?;
            } else {
                self.edit_photo(id, &value, label)?;
            }
            count += 1;
        }
        Ok(count)
    }

    fn edit_photos_batch(&mut self, ids: &[u64], settings: &[Settings], label: &str) -> Result<Vec<u64>, String> {
        self.write_batch(ids, settings, label, true)
    }

    fn edit_photos_batch_own(&mut self, ids: &[u64], settings: &[Settings], label: &str) -> Result<Vec<u64>, String> {
        self.write_batch(ids, settings, label, false)
    }

    fn copied(&self) -> Option<Settings> {
        self.copied.clone()
    }

    fn take_commits(&mut self) -> Vec<(u64, Commit)> {
        std::mem::take(&mut self.commits)
    }

    fn copy(&mut self) -> bool {
        if self.open.is_none() {
            return false;
        }
        self.copied = Some(self.edit.settings().clone());
        true
    }

    fn has_copied(&self) -> bool {
        self.copied.is_some()
    }

    fn paste(&mut self, targets: &[usize]) -> Result<usize, String> {
        let Some(copied) = self.copied.clone() else { return Ok(0) };
        self.edit.commit("보정 변경");
        Document::flush(self)?;
        let mut count = 0;
        let current = self.open.as_ref().map(|o| o.id);
        for &index in targets {
            let Some(id) = self.listing.photos.get(index).map(|p| p.id) else { continue };
            count += 1;
            if Some(id) == current {
                let value = crate::session::pasted_settings(&copied, self.edit.settings())?;
                self.edit.replace(value, "보정 붙여넣기");
                Document::flush(self)?;
            } else {
                let value = crate::session::pasted_settings(&copied, &self.stored_settings(id)?)?;
                self.edit_photo(id, &value, "보정 붙여넣기")?;
            }
        }
        Ok(count)
    }

    fn set_label_field(&mut self, field: LabelField, value: Value) -> Result<(), String> {
        let ids = self.selected_ids();
        if ids.is_empty() {
            return Ok(());
        }
        if self.read_only {
            for id in &ids {
                let overlay = self.overlay.entry(*id).or_default();
                match field {
                    LabelField::Rating => overlay.rating = value.as_i64(),
                    LabelField::Flag => overlay.flag = value.as_i64(),
                    LabelField::Label => overlay.label = value.as_str().map(str::to_owned),
                }
            }
        } else {
            let ids: Vec<i64> = ids.iter().map(|i| *i as i64).collect();
            self.catalog.update_many(&ids, field.column(), &value).map_err(err)?;
        }
        for photo in self.listing.photos.iter_mut().chain(self.open.iter_mut()) {
            if ids.contains(&photo.id) {
                match field {
                    LabelField::Rating => photo.rating = value.as_i64().unwrap_or(0),
                    LabelField::Flag => photo.flag = value.as_i64().unwrap_or(0),
                    LabelField::Label => photo.label = value.as_str().unwrap_or("").to_owned(),
                }
            }
        }
        // `refresh_lists()`: a filter may hide the photo now.
        self.request_refresh(false);
        Ok(())
    }

    fn add_files(&mut self, files: Vec<PathBuf>, roots: &[PathBuf]) -> Result<usize, String> {
        if self.read_only {
            return Err("read-only".into());
        }
        let cancel = AtomicBool::new(false);
        let report = import::import_files(&self.catalog, &files, roots, &cancel, &|_, _, _| {})?;
        self.request_refresh(self.open.is_none());
        Ok(report.added.len())
    }

    fn remove_selected(&mut self) -> Result<usize, String> {
        if self.read_only {
            return Err("read-only".into());
        }
        self.edit.commit("보정 변경");
        Document::flush(self)?;
        let ids: Vec<i64> = self.selected_ids().iter().map(|i| *i as i64).collect();
        if ids.is_empty() {
            return Ok(0);
        }
        self.clear_active_photo();
        let paths: Vec<String> = ids
            .iter()
            .filter_map(|id| self.catalog.photo(*id).ok().flatten())
            .filter_map(|row| row["path"].as_str().map(str::to_owned))
            .collect();
        self.catalog.remove_photos(&ids).map_err(err)?;
        // `manager.remove(False)`: files that stay are not brought back by the automatic folder import.
        let left: BTreeSet<String> = self.catalog.paths().map_err(err)?.into_iter().collect();
        let gone: Vec<String> = paths.into_iter().filter(|p| !left.contains(p)).collect();
        let mut sync = grainy_library::folder_sync::FolderSync::new(&self.catalog, || 0.0).map_err(err)?;
        sync.exclude(&self.catalog, &gone).map_err(err)?;
        self.listing.photos.retain(|p| !ids.contains(&(p.id as i64)));
        self.request_refresh(true);
        Ok(ids.len())
    }

    fn query(&self) -> Query {
        self.query.clone()
    }

    fn set_query(&mut self, query: Query) -> Result<(), String> {
        if query != self.query {
            self.query = query;
            self.request_refresh(true);
        }
        Ok(())
    }

    fn refresh(&mut self) -> Result<(), String> {
        self.refresh_now()
    }

    fn folders(&self) -> Vec<FolderNode> {
        self.listing.folders.clone()
    }

    fn collections(&self) -> Vec<Collection> {
        self.listing.collections.clone()
    }

    fn library_folder(&self) -> Option<PathBuf> {
        (!self.read_only).then(|| self.folder.clone())
    }

    fn listing_job(&mut self) -> Option<ListingJob> {
        CatalogDocument::listing_job(self)
    }

    fn apply_listing(&mut self, listing: Listing) {
        CatalogDocument::apply_listing(self, listing);
    }

    fn listing_pending(&self) -> bool {
        CatalogDocument::listing_pending(self)
    }

    fn request_refresh(&mut self, ensure_current: bool) {
        CatalogDocument::request_refresh(self, ensure_current);
    }

    fn collection_op(&mut self, op: collections::CollectionOp) -> Result<collections::CollectionReply, String> {
        collections::apply(self, op)
    }

    fn virtual_copies(&mut self) -> Result<usize, String> {
        CatalogDocument::virtual_copies(self)
    }

    fn stack_op(&mut self, op: stacks::StackOp) -> Result<stacks::StackReply, String> {
        stacks::apply(self, op)
    }

    fn file_op(&mut self, op: files::FileOp) -> Result<files::FileReply, String> {
        files::apply(self, op)
    }

    fn preference(&self, key: &str) -> Option<Value> {
        self.catalog.preference(key, Value::Null).ok().filter(|v| !v.is_null())
    }

    fn save_preference(&mut self, key: &str, value: &Value) -> Result<(), String> {
        if self.read_only {
            return Ok(());
        }
        self.catalog.save_preference(key, value).map_err(err)
    }

    fn folder_inputs(&self) -> (u64, &FolderInputs) {
        (self.applied, &self.listing.folder_inputs)
    }

    fn folder_photo_count(&self, folder: &str) -> usize {
        self.catalog.folder_photo_ids(folder).map_or(0, |ids| ids.len())
    }

    fn remove_folder(&mut self, folder: &str) -> Result<usize, String> {
        if self.read_only {
            return Err("read-only".into());
        }
        if Path::new(folder).parent().is_none() {
            return Ok(0);
        }
        self.edit.commit("보정 변경");
        Document::flush(self)?;
        let stamp = local_now().format("%Y%m%d-%H%M%S");
        let backup = self.folder.join("backups").join(format!("folder-removal-{stamp}-{}.sqlite", unique_suffix()));
        self.catalog.backup(&backup).map_err(err)?;
        let ids = self.catalog.folder_photo_ids(folder).map_err(err)?;
        if ids.contains(&(self.current_id() as i64)) {
            self.clear_active_photo();
        }
        let removed = self.catalog.remove_folder(folder).map_err(err)?;
        self.listing.photos.retain(|p| !removed.contains(&(p.id as i64)));
        self.request_refresh(true);
        Ok(removed.len())
    }

    fn register_folder(&mut self, folder: &str) -> Result<(), String> {
        if self.read_only {
            return Ok(());
        }
        self.catalog.register_folder(folder, false).map(|_| ()).map_err(err)
    }

    fn start_without_photo(&mut self) {
        self.ensure_current = false;
        self.hold_first = true;
        self.restore = None;
    }

    fn restore_photo(&mut self, id: Option<u64>) {
        self.restore = id;
    }

    fn store_metadata(&mut self, id: u64, metadata: &Value) -> Result<(), String> {
        if self.read_only {
            return Ok(());
        }
        let mut fields = Map::new();
        fields.insert("metadata".into(), Value::String(grainy_catalog::json::dumps(metadata, true, false, false)));
        self.catalog.update(id as i64, &fields).map_err(err)
    }

    fn metadata_catalog(&self) -> Option<&Catalog> {
        Some(&self.catalog)
    }

    fn refresh_command_edits(&mut self, ids: &[u64]) -> Result<(), String> {
        for &id in ids {
            self.bump(id);
        }
        let open = self.open.as_ref().map(|o| o.id);
        if let Some(id) = open.filter(|id| ids.contains(id)) {
            let version = self.edit.version();
            let mut state = self.load(id)?;
            let live = state.settings().clone();
            while state.version() <= version {
                state.set_all(live.clone());
            }
            self.edit = state;
            self.refresh_history();
        }
        Ok(())
    }
}

/// Eight hex digits that differ between calls (`uuid4().hex[:8]` of the backup names).
fn unique_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as u64);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9e37_79b9);
    let mixed = nanos ^ (u64::from(std::process::id()) << 32) ^ count;
    format!("{:08x}", (mixed ^ (mixed >> 29)).wrapping_mul(0xbf58_476d_1ce4_e5b9) >> 32)
}

/// The import (`import_paths` / `import_next`, add in place): finding the photo files and adding them.
pub mod import {
    use super::*;
    use grainy_codecs::ColorManager;

    /// What an import did.
    #[derive(Debug, Clone, Default)]
    pub struct Report {
        pub added: Vec<i64>,
        /// "name: reason" per file that could not be read (`import_errors`).
        pub errors: Vec<String>,
    }

    /// Port of `import_paths`' scan: the photo files under `paths` (folders with all their subfolders,
    /// `Path.rglob('*')`), sorted as text and without duplicates, and the folders to register (each folder
    /// resolved; a file's own folder).
    pub fn scan(paths: &[PathBuf], cancel: &AtomicBool) -> (Vec<PathBuf>, Vec<PathBuf>) {
        scan_excluding(paths, cancel, None)
    }

    /// Scans selected paths without reimporting this catalogue's generated JPEG thumbnails.
    pub(crate) fn scan_for_library(
        paths: &[PathBuf],
        cancel: &AtomicBool,
        library: &Path,
    ) -> (Vec<PathBuf>, Vec<PathBuf>) {
        scan_excluding(paths, cancel, Some(&resolve(&library.join("thumbnails"))))
    }

    fn scan_excluding(paths: &[PathBuf], cancel: &AtomicBool, cache: Option<&Path>) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let mut files = BTreeSet::new();
        let mut roots = Vec::new();
        fn walk(folder: &Path, out: &mut BTreeSet<String>, cancel: &AtomicBool, cache: Option<&Path>) {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                return;
            }
            let Ok(entries) = std::fs::read_dir(folder) else { return };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(kind) = entry.file_type() else { continue };
                if kind.is_dir() {
                    if !cache.is_some_and(|cache| resolve(&path) == cache) {
                        walk(&path, out, cancel, cache);
                    }
                } else if super::super::has_extension(&path, &grainy_service::IMAGE_EXTENSIONS) && path.is_file() {
                    out.insert(path.to_string_lossy().into_owned());
                }
            }
        }
        for path in paths {
            let resolved = resolve(path);
            if cache.is_some_and(|cache| resolved.starts_with(cache)) {
                continue;
            }
            if path.is_dir() {
                roots.push(resolved);
                walk(path, &mut files, cancel, cache);
            } else if super::super::has_extension(path, &grainy_service::IMAGE_EXTENSIONS) {
                if let Some(parent) = resolved.parent() {
                    roots.push(parent.to_path_buf());
                }
                files.insert(path.to_string_lossy().into_owned());
            }
        }
        (files.into_iter().map(PathBuf::from).collect(), roots)
    }

    fn resolve(path: &Path) -> PathBuf {
        let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let text = real.to_string_lossy();
        match text.strip_prefix(r"\\?\UNC\") {
            Some(unc) => PathBuf::from(format!(r"\\{unc}")),
            None => PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()),
        }
    }

    /// The thumbnail and metadata of a new photo: `engine.thumbnail(path, 240, with_info=True)` for ordinary
    /// files; `raw_defaults.import_preview(path, rules)` for RAW files (the built-in RAW development and the
    /// import rules, with the settings).
    pub fn preview(path: &Path, rules: &[Value]) -> Result<(ndarray::Array3<u8>, Value, Option<Settings>), String> {
        if super::super::has_extension(path, &grainy_service::RAW_EXTENSIONS) {
            let mut info = match grainy_raw::metadata::read_metadata(path) {
                Ok(Value::Object(map)) => map,
                _ => Map::new(),
            };
            let format = path.extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default();
            info.insert("format".into(), Value::String(format));
            info.insert("linear_raw".into(), Value::Bool(grainy_raw::raw_defaults::linear_raw(path)));
            let (settings, notes) =
                grainy_raw::raw_defaults::resolve(&Value::Object(info), rules, Some(path), None).map_err(err)?;
            let loaded = grainy_engine::files::load_photo(path, Some(240), &settings).map_err(err)?;
            let mut metadata = loaded.metadata;
            if !notes.is_empty() {
                metadata.insert("raw_default_applied".into(), json!(notes));
            }
            let space = if settings.str("working_space") == "ProPhoto" { "ProPhoto RGB" } else { "sRGB" };
            metadata.insert("preview_color_space".into(), json!(space));
            let size = |k: &str| metadata.get(k).and_then(Value::as_f64).unwrap_or(0.0);
            let options = grainy_engine::Options {
                output: grainy_engine::Output::Named(space),
                original_size: Some((size("width"), size("height"))),
                ..grainy_engine::Options::default()
            };
            let image = grainy_engine::develop(loaded.photo.source(), &settings.0, &options).map_err(err)?;
            let pixels = image.mapv(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
            return Ok((pixels, Value::Object(metadata), Some(settings)));
        }
        let (pixels, metadata) =
            grainy_codecs::thumbnail(path, 240, &grainy_engine::files::Cms::for_loading()).map_err(err)?;
        Ok((pixels, Value::Object(metadata), None))
    }

    /// Runs a catalogue write again while SQLite says the database is locked. The oracle writes from one
    /// connection on the window's thread; here the import has its own connection beside the window's, and a
    /// deferred transaction that read before another connection wrote cannot be upgraded (`SQLITE_BUSY`).
    fn retrying<T>(mut write: impl FnMut() -> grainy_catalog::Result<T>) -> grainy_catalog::Result<T> {
        let mut tries = 0;
        loop {
            match write() {
                Err(error) if tries < 100 && error.to_string().contains("database is locked") => {
                    tries += 1;
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                other => return other,
            }
        }
    }

    /// Port of `import_paths` (after the scan) and `import_next`: registers the folders (`register_folder`,
    /// `restore_folder`), then adds every file the library does not have yet (`Catalog.add` with the import's
    /// metadata) and stores its first thumbnail (`preview_store.store_render`). `progress(done, total, name)`
    /// after each file. Original files are only read.
    pub fn import_files(
        catalog: &Catalog,
        files: &[PathBuf],
        roots: &[PathBuf],
        cancel: &AtomicBool,
        progress: &dyn Fn(usize, usize, &str),
    ) -> Result<Report, String> {
        let platform = catalog.platform();
        for root in roots {
            let root = root.to_string_lossy();
            retrying(|| catalog.register_folder(&root, false)).map_err(err)?;
            retrying(|| catalog.restore_folder(&root)).map_err(err)?;
        }
        let mut known: BTreeSet<String> = catalog.paths().map_err(err)?.iter().map(|p| platform.path_key(p)).collect();
        let rules: Vec<Value> = catalog
            .preference("raw_defaults_v1", json!([]))
            .ok()
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let queue: Vec<&PathBuf> =
            files.iter().filter(|f| known.insert(platform.path_key(&resolve(f).to_string_lossy()))).collect();
        let mut report = Report::default();
        // `folder_sync.imported(path)`: a photo brought back leaves the automatic import's excluded list.
        let mut sync = grainy_library::folder_sync::FolderSync::new(catalog, || 0.0).ok();
        let srgb =
            grainy_engine::files::Cms::for_loading().profile_bytes(&grainy_codecs::Profile::Named("sRGB".into())).ok();
        for (done, file) in queue.iter().enumerate() {
            if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            progress(done, queue.len(), &name);
            let (pixels, info, settings) = match preview(file, &rules) {
                Ok(result) => result,
                Err(reason) => {
                    report.errors.push(format!("{name}: {}", crate::session_state::import_reason(file, &reason)));
                    continue;
                }
            };
            // The oracle invalidates thumbnails after storing the import preview.
            // Prevent a worker from retaining an interim rebuild before that packet exists.
            let _publication = crate::thumbjpeg::publication();
            let id = retrying(|| catalog.add(&file.to_string_lossy(), settings.as_ref(), Some(&info), false))
                .map_err(err)?;
            report.added.push(id);
            if let Some(sync) = sync.as_mut() {
                if let Err(error) = sync.imported(catalog, &file.to_string_lossy(), false) {
                    log::warn!("sync_excluded not updated for {name}: {error}");
                }
            }
            let saved = settings.unwrap_or_else(|| normalized(&Map::new()));
            let space = info["preview_color_space"].as_str().unwrap_or("sRGB").to_owned();
            let icc = if space == "sRGB" { srgb.clone() } else { None };
            match crate::thumbjpeg::encode(&pixels, icc) {
                Ok(jpeg) => {
                    let source = info.get("file_fingerprint").cloned().unwrap_or(Value::Null);
                    let stored = grainy_catalog::preview::store_render(
                        catalog.directory(),
                        id,
                        &saved,
                        &jpeg,
                        &source,
                        &Value::Null,
                        &space,
                    );
                    if let Err(error) = stored {
                        log::warn!("thumbnail of {name} not stored: {error}");
                    }
                }
                Err(error) => log::warn!("thumbnail of {name} not encoded: {error}"),
            }
        }
        progress(queue.len(), queue.len(), "");
        Ok(report)
    }
}

#[cfg(test)]
#[test]
fn failed_undo_write_retains_edit_and_rolls_back_history_before_retry() {
    let tmp = tempfile::tempdir().unwrap();
    let mut doc = CatalogDocument::open(tmp.path()).unwrap();
    let first = doc.catalog.add("first.png", None, None, true).unwrap();
    doc.catalog.add("second.png", None, None, true).unwrap();
    doc.refresh_now().unwrap();
    doc.select(0);
    assert_eq!(doc.current_id(), first as u64);
    let mut settings = doc.edit().settings().clone();
    settings.set("exposure", json!(1.5));
    doc.edit_mut().replace(settings, "exposure");
    doc.catalog
        .connection()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_undo BEFORE INSERT ON preferences WHEN NEW.key LIKE 'undo:%'
         BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
        )
        .unwrap();
    assert!(doc.flush().is_err());
    assert!(!doc.select(1));
    assert!(doc.edit().has_journal());
    assert!(doc.catalog.histories(first).unwrap().is_empty());
    doc.catalog.connection().execute_batch("DROP TRIGGER fail_undo").unwrap();
    doc.flush().unwrap();
    doc.flush().unwrap();
    assert_eq!(doc.catalog.histories(first).unwrap().len(), 1);
    assert_eq!(doc.catalog.photo(first).unwrap().unwrap()["settings"]["exposure"], json!(1.5));
}
