//! The document model: the list of photos on screen, the selection, the open photo's [`EditState`] and where its
//! commits go. Two implementations: [`crate::session::Session`] (photos opened in memory, nothing is written) and
//! [`catalog::CatalogDocument`] (a library on disk, the oracle's SQLite catalogue through `grainy-catalog`).
//!
//! Features never ask which one is open: they edit `doc.edit_mut()` (set, commit, undo…), call [`Document::flush`]
//! (the application does it every frame) and read the list through this trait.

pub mod catalog;

use std::path::{Path, PathBuf};

use grainy_core::Settings;
use serde_json::Value;

use crate::state::{Commit, EditState};

/// One photo of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoEntry {
    /// The document's id: a session number from 1, or the catalogue's photo id (0 means no photo).
    pub id: u64,
    pub path: PathBuf,
    /// The name shown (a virtual copy's name ends in " · 사본").
    pub name: String,
    /// 0–5 stars.
    pub rating: i64,
    /// 1 picked, -1 rejected, 0 none.
    pub flag: i64,
    /// The colour label's Korean name (`빨강`…), or empty.
    pub label: String,
    /// The original's id when this is a virtual copy.
    pub virtual_source: Option<u64>,
    /// The stack it belongs to (`stack_id`).
    pub stack: Option<i64>,
    /// The file's oriented, uncropped size, once known.
    pub original: Option<(u32, u32)>,
}

impl PhotoEntry {
    /// A photo with only a path (a session photo, a row before its page is read).
    pub fn new(id: u64, path: PathBuf) -> Self {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Self {
            id,
            path,
            name,
            rating: 0,
            flag: 0,
            label: String::new(),
            virtual_source: None,
            stack: None,
            original: None,
        }
    }

    /// Whether the file is a RAW file (by its extension).
    pub fn is_raw(&self) -> bool {
        has_extension(&self.path, &grainy_service::RAW_EXTENSIONS)
    }

    /// Port of `PhotoModel.caption`: "✓ name" / "× name" and the stars.
    pub fn caption(&self) -> (String, String) {
        let prefix = match self.flag {
            1 => "✓ ",
            -1 => "× ",
            _ => "",
        };
        (format!("{prefix}{}", self.name), "★".repeat(self.rating.clamp(0, 5) as usize))
    }
}

pub(crate) fn has_extension(path: &Path, list: &[&str]) -> bool {
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default();
    list.contains(&ext.as_str())
}

/// Where the photos live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// Opened in memory; nothing is written.
    Session,
    /// A library folder; `read_only` for the Python Grainy's library (edits stay in memory).
    Catalog { folder: PathBuf, read_only: bool },
}

/// The library's list options (`refresh_lists`: the filter buttons, search, folder, collection, sort, stacks).
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    /// `all`, `picked`, `rated` or `rejected`.
    pub mode: String,
    pub search: String,
    /// The folder chosen in the tree.
    pub folder: Option<String>,
    /// 하위 폴더 사진 포함.
    pub recursive: bool,
    /// The collection chosen.
    pub collection: Option<i64>,
    /// Smart filter rules (상세 필터), `{}` when none.
    pub rules: Value,
    /// `imported`, `name`, `capture` or `rating`.
    pub sort: String,
    pub descending: bool,
    /// 스택 접어서 보기.
    pub collapsed: bool,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            mode: "all".into(),
            search: String::new(),
            folder: None,
            recursive: true,
            collection: None,
            rules: Value::Object(Default::default()),
            sort: "imported".into(),
            descending: false,
            collapsed: false,
        }
    }
}

impl Query {
    /// The options of `library_query.query` (`refresh_lists`), with the saved stack orders.
    pub fn options(&self, stack_orders: Value) -> Value {
        // `recursive=include_subfolders or folder is a drive root`.
        let root = self.folder.as_deref().is_some_and(|f| Path::new(f).parent().is_none());
        serde_json::json!({
            "search": self.search,
            "folder": self.folder,
            "recursive": self.recursive || root,
            "mode": self.mode,
            "rules": self.rules,
            "collection": self.collection,
            "collapsed": self.collapsed,
            "stack_orders": stack_orders,
            "sort": self.sort,
            "descending": self.descending,
        })
    }
}

/// A folder of the library's tree (`folders.folder_nodes`).
#[derive(Debug, Clone, PartialEq)]
pub struct FolderNode {
    pub path: String,
    pub name: String,
    /// Photos directly in it and with its subfolders.
    pub direct: i64,
    pub total: i64,
    pub children: Vec<FolderNode>,
}

/// A collection (`Catalog.collections`).
#[derive(Debug, Clone, PartialEq)]
pub struct Collection {
    pub id: i64,
    pub name: String,
    pub parent: Option<i64>,
    /// Smart collections have rules.
    pub smart: bool,
}

/// One row of the History list (보정 기록): what it shows and the settings a double click restores.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryEntry {
    pub label: String,
    pub created: String,
    pub settings: Option<Settings>,
}

/// The field [`Document::set_label_field`] changes on the selected photos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelField {
    Rating,
    Flag,
    Label,
}

impl LabelField {
    /// The catalogue column.
    pub fn column(self) -> &'static str {
        match self {
            LabelField::Rating => "rating",
            LabelField::Flag => "flag",
            LabelField::Label => "label",
        }
    }
}

/// Where a thumbnail can be read from and stored (`preview_store`): the library folder and the photo's id.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ThumbCache {
    /// Private catalogue snapshot for a read-only library.
    pub snapshot: Option<PathBuf>,
    pub directory: PathBuf,
    pub id: i64,
    /// Rendered thumbnails are published to the cache (never for a read-only library).
    pub writable: bool,
}

/// What a listing read gives: the photos in order, the library's count, the folder tree and the collections.
#[derive(Debug, Clone, Default)]
pub struct Listing {
    /// The refresh it answers.
    pub generation: u64,
    pub photos: Vec<PhotoEntry>,
    pub total: usize,
    pub folders: Vec<FolderNode>,
    pub collections: Vec<Collection>,
    /// What the folder tree is built from (`result['folders']`, `result['roots']` and the catalogue's removed
    /// folders): the library column makes its own tree of it (missing and unscanned marks, scanned subfolders).
    pub folder_inputs: FolderInputs,
}

/// The inputs of `folders.folder_nodes` for the library column.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FolderInputs {
    /// The folder rows of the query (`path` of a file in the folder and `count`).
    pub rows: Vec<Value>,
    /// The registered folders (`Catalog.folder_roots`).
    pub roots: Vec<String>,
    /// The folders taken out of the catalogue (`Catalog.removed_folders`).
    pub removed: Vec<String>,
}

/// The folder inputs of a document without folders.
pub static NO_FOLDERS: FolderInputs = FolderInputs { rows: Vec::new(), roots: Vec::new(), removed: Vec::new() };

/// A listing to read off the window's thread: run it, then give the result to [`CatalogDocument::apply_listing`].
pub type ListingJob = Box<dyn FnOnce() -> Result<Listing, String> + Send>;

/// The list of photos, the selection, the open photo's edits, and where they are stored.
pub trait Document {
    /// Session or catalogue.
    fn kind(&self) -> Kind;

    /// Edits stay in memory (the session, or a library opened read-only).
    fn read_only(&self) -> bool {
        !matches!(self.kind(), Kind::Catalog { read_only: false, .. })
    }

    /// The photos of the list, in order.
    fn photos(&self) -> &[PhotoEntry];

    /// How many photos.
    fn len(&self) -> usize {
        self.photos().len()
    }

    /// Whether the list is empty.
    fn is_empty(&self) -> bool {
        self.photos().is_empty()
    }

    /// How many photos the whole library holds (the list may be filtered).
    fn total(&self) -> usize {
        self.len()
    }

    /// The open photo's index in the list.
    fn current_index(&self) -> Option<usize>;

    /// The open photo.
    fn current(&self) -> Option<&PhotoEntry> {
        self.current_index().and_then(|i| self.photos().get(i))
    }

    /// The open photo's id, 0 when none.
    fn current_id(&self) -> u64 {
        self.current().map_or(0, |p| p.id)
    }

    /// The index of a photo id.
    fn index_of(&self, id: u64) -> Option<usize> {
        self.photos().iter().position(|p| p.id == id)
    }

    /// Opens photo `index` (`activate`): the open photo's pending change is committed first; the selection becomes
    /// that photo. Returns whether the open photo changed.
    fn select(&mut self, index: usize) -> bool;

    /// Opens the photo `delta` places away (arrow keys), stopping at the ends.
    fn step(&mut self, delta: isize) -> bool {
        let Some(current) = self.current_index() else { return false };
        let target = current.saturating_add_signed(delta).min(self.len().saturating_sub(1));
        self.select(target)
    }

    /// Ctrl/Cmd-click: adds or removes a photo from the selection (the open photo stays selected).
    fn toggle_selected(&mut self, index: usize);

    /// Shift-click (or Ctrl/Cmd+Shift-click when `additive`): selects the contiguous range from the
    /// selection anchor (or open photo) to `index`, keeping the open photo selected.
    fn select_range(&mut self, index: usize, additive: bool);

    /// Ctrl/Cmd+A: selects every photo in the current listing.
    fn select_all(&mut self);

    /// Ctrl/Cmd+D: clears multi-selection back to only the open photo.
    fn deselect_others(&mut self);

    /// Whether a photo is selected.
    fn is_selected(&self, id: u64) -> bool;

    /// The selected photos' indices in order (at least the open photo): `selected_ids`.
    /// Whether more than one visible photo is selected.
    fn multiple_selected(&self) -> bool {
        self.selected_indices().len() > 1
    }

    fn selected_indices(&self) -> Vec<usize>;

    /// The open photo's edit state (a scratch state when none is open).
    fn edit(&self) -> &EditState;

    /// The open photo's edit state, to change.
    fn edit_mut(&mut self) -> &mut EditState;

    /// The settings of photo `index` (the live ones for the open photo).
    fn settings_of(&self, index: usize) -> Settings;

    /// Changes whenever photo `index`'s settings change (thumbnail keys).
    fn version_of(&self, index: usize) -> u64;

    /// Where photo `index`'s thumbnail is cached, if anywhere.
    fn thumb_cache(&self, _index: usize) -> Option<ThumbCache> {
        None
    }

    /// Records a photo's original size.
    fn set_original(&mut self, id: u64, size: (u32, u32));

    /// Stores what the edit state recorded (commits, undo, redo). Called every frame and before anything that reads
    /// the stored settings. Returns the message of a failed write.
    fn flush(&mut self) -> Result<(), String>;

    /// The History list of the open photo, newest first.
    fn history(&self) -> Vec<HistoryEntry>;

    /// Port of `restore_history`: the settings of history row `index`, as one step.
    fn restore_history(&mut self, index: usize) -> bool {
        let Some(settings) = self.history().get(index).and_then(|h| h.settings.clone()) else { return false };
        let edit = self.edit_mut();
        edit.commit("보정 변경");
        edit.replace(settings, "보정 변경");
        true
    }

    /// Port of `copy_edits`: copies the open photo's settings. False when no photo is open.
    fn copy(&mut self) -> bool;

    /// Whether settings were copied.
    fn has_copied(&self) -> bool;

    /// Port of `paste_edits`: the copied settings into each photo of `targets` (indices), one step "보정 붙여넣기"
    /// per photo. Returns how many photos.
    fn paste(&mut self, targets: &[usize]) -> Result<usize, String>;

    // ---- sync and copy settings (work package 12) ------------------------------------------------------------

    /// The stored settings of photo `id` (`catalog.photo(id)['settings']`): the live ones for the open photo.
    fn stored_settings_of(&self, id: u64) -> Result<Settings, String>;

    /// The stored `info` of photo `id` (`catalog.photo(id)['info']`); a photo without one has its file format.
    fn photo_info(&self, id: u64) -> Value {
        let format = self
            .index_of(id)
            .and_then(|i| self.photos().get(i))
            .and_then(|p| p.path.extension().map(|e| e.to_string_lossy().to_lowercase()))
            .unwrap_or_default();
        serde_json::json!({ "format": format })
    }

    /// Port of `LibraryManager.edit_photo` for each photo: `settings[i]` (fitted to the photo's RAW state with
    /// `rawcolor.retarget`) becomes photo `ids[i]`'s edits as one history row `label` and one undo step of its own
    /// (`undo:{id}`); the open photo goes through its edit state. Photos whose settings do not change write
    /// nothing. Returns how many photos it went through.
    fn edit_photos(&mut self, ids: &[u64], settings: &[Settings], label: &str) -> Result<usize, String>;

    /// Port of `command_batch.apply(..., remember=False, switch=False)` as `WorkflowTools.apply_edits(...,
    /// retarget=True)` calls it: the values fitted with `rawcolor.retarget`, all written in one transaction (a
    /// history row `label` and an undo step per changed photo, no batch record). The open photo's pending change
    /// is stored first and its state read again. Returns the changed ids.
    fn edit_photos_batch(&mut self, ids: &[u64], settings: &[Settings], label: &str) -> Result<Vec<u64>, String>;

    /// [`Document::edit_photos_batch`] for values computed from each photo's own settings
    /// (`WorkflowTools.apply_edits(..., retarget=False)`: 카메라 방향 복원): the RAW colour settings are written as
    /// they are, not fitted to the file again.
    fn edit_photos_batch_own(&mut self, ids: &[u64], settings: &[Settings], label: &str) -> Result<Vec<u64>, String> {
        self.edit_photos_batch(ids, settings, label)
    }

    /// The settings `copy` took (`MainWindow.copied`).
    fn copied(&self) -> Option<Settings> {
        None
    }

    /// The commits of the open photo since the last call, with its id (`LibraryManager.after_commit`'s input).
    fn take_commits(&mut self) -> Vec<(u64, Commit)> {
        Vec::new()
    }

    /// Port of `set_rating` / `set_flag` / `manager.set_label`: `field` set to `value` on the selected photos.
    fn set_label_field(&mut self, field: LabelField, value: Value) -> Result<(), String>;

    /// Adds photo files that are already found (the import's scan ran elsewhere). Returns how many were added.
    fn add_files(&mut self, files: Vec<PathBuf>, roots: &[PathBuf]) -> Result<usize, String>;

    /// Port of `remove_from_catalog` ("카탈로그에서만 제거"): the selected photos leave the list and the library.
    /// Files are never touched. Returns how many.
    fn remove_selected(&mut self) -> Result<usize, String>;

    /// The list options.
    fn query(&self) -> Query {
        Query::default()
    }

    /// Changes the list options and refreshes the list.
    fn set_query(&mut self, _query: Query) -> Result<(), String> {
        Ok(())
    }

    /// Reads the list again (after an import, a rating under a filter…).
    fn refresh(&mut self) -> Result<(), String> {
        Ok(())
    }

    /// The folder tree.
    fn folders(&self) -> Vec<FolderNode> {
        Vec::new()
    }

    /// The collections.
    fn collections(&self) -> Vec<Collection> {
        Vec::new()
    }
    /// The library folder the import job writes to (`None` for the session and a read-only library).
    fn library_folder(&self) -> Option<PathBuf> {
        None
    }

    /// The directory holding a library's photo assets, including those of a read-only catalogue.
    fn asset_folder(&self) -> Option<PathBuf> {
        match self.kind() {
            Kind::Catalog { folder, .. } => Some(folder),
            _ => None,
        }
    }

    /// The listing to read in the background, once per refresh asked for.
    fn listing_job(&mut self) -> Option<ListingJob> {
        None
    }

    /// Shows a listing read by [`Document::listing_job`].
    fn apply_listing(&mut self, _listing: Listing) {}

    /// Whether a listing was asked for and not shown yet.
    fn listing_pending(&self) -> bool {
        false
    }

    /// A collection operation (work package 15: `catalog::collections`); a session has no collections.
    fn collection_op(
        &mut self,
        _op: catalog::collections::CollectionOp,
    ) -> Result<catalog::collections::CollectionReply, String> {
        Err("collections need a library".into())
    }

    /// Asks for the list to be read again in the background (`ensure_current`: open the first photo when the open
    /// one is gone).
    fn request_refresh(&mut self, _ensure_current: bool) {}

    /// Port of `manager.virtual_copy`: a virtual copy of each selected photo. Returns how many.
    fn virtual_copies(&mut self) -> Result<usize, String> {
        Err("가상 사본은 라이브러리에서만 만들 수 있습니다.".into())
    }

    /// A stack or snapshot operation (work package 16: `catalog::stacks`); a session has neither.
    fn stack_op(&mut self, _op: catalog::stacks::StackOp) -> Result<catalog::stacks::StackReply, String> {
        Err("스택과 스냅샷은 라이브러리에서만 쓸 수 있습니다.".into())
    }

    /// A file-management operation (work package 22: `catalog::files`); a session has no catalogue.
    fn file_op(&mut self, _op: catalog::files::FileOp) -> Result<catalog::files::FileReply, String> {
        Err("라이브러리에서만 할 수 있습니다.".into())
    }

    // ---- the library column and the saved session state (work package 1) --------------------------------------

    /// A stored preference of the library (`Catalog.preference`), `None` when it is not set or the document has
    /// no catalogue.
    fn preference(&self, _key: &str) -> Option<Value> {
        None
    }

    /// Stores a preference (`Catalog.save_preference`). A session and a read-only library write nothing.
    fn save_preference(&mut self, _key: &str, _value: &Value) -> Result<(), String> {
        Ok(())
    }

    /// The inputs of the folder tree of the listing shown, and which listing that is.
    fn folder_inputs(&self) -> (u64, &FolderInputs) {
        (0, &NO_FOLDERS)
    }

    /// How many photos (virtual copies included) are in `folder` and below (`len(catalog.folder_photo_ids)`).
    fn folder_photo_count(&self, _folder: &str) -> usize {
        0
    }

    /// Port of `FolderPanel.remove` after the question: the pending change is stored, the catalogue is backed up
    /// to `backups/folder-removal-….sqlite`, the folder and the photos below it leave the catalogue (files are
    /// never touched) and the list is read again. Returns how many photos.
    fn remove_folder(&mut self, _folder: &str) -> Result<usize, String> {
        Err("라이브러리에서만 할 수 있습니다.".into())
    }

    /// Port of `show_parent_folder`: registers the parent of `folder` as a root (`Catalog.register_folder`).
    fn register_folder(&mut self, _folder: &str) -> Result<(), String> {
        Ok(())
    }

    /// Safe mode (the run before did not close): no photo opens by itself (`restore_folder_selection`).
    fn start_without_photo(&mut self) {}

    /// Port of `restore_photo_id`: the photo to open when the next listing shows it.
    fn restore_photo(&mut self, _id: Option<u64>) {}

    /// Port of `activate`'s `catalog.update(photo_id, metadata=json.dumps(info))`: the metadata read when the
    /// photo was loaded replaces the stored one.
    fn store_metadata(&mut self, _id: u64, _metadata: &Value) -> Result<(), String> {
        Ok(())
    }

    /// The catalogue behind the list, for the metadata panel and commands (work package 6): `None` for a session.
    /// A read-only library gives its reader (reads work, writes are refused).
    fn metadata_catalog(&self) -> Option<&grainy_catalog::Catalog> {
        None
    }

    /// Port of `MainWindow.refresh_command_edits` (work package 25): photos a command batch wrote to the catalogue
    /// are read again (thumbnails, and the open photo's state and undo stacks).
    fn refresh_command_edits(&mut self, _ids: &[u64]) -> Result<(), String> {
        Ok(())
    }

    /// Photos changed in the catalogue by someone else (the AI connection, work package 30): their thumbnails are
    /// made again and, when the open photo is among them, its state and history are read again
    /// (`refresh_command_edits`).
    fn changed_elsewhere(&mut self, ids: &[u64]) {
        if let Err(message) = self.refresh_command_edits(ids) {
            log::warn!("cannot read photos {ids:?} again: {message}");
        }
    }
}
