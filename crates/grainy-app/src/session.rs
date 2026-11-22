//! The photos opened in this session, in memory only (no catalogue is opened or written): each keeps its own
//! [`EditState`] (edits and undo survive switching photos), the selection, and the copied settings. The
//! [`Document`] of `--memory` runs and of the shell's tests.

use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use grainy_core::{normalized, Settings};
use serde_json::{Map, Value};

use crate::document::{has_extension, Document, HistoryEntry, Kind, LabelField, PhotoEntry};
use crate::state::{Commit, EditState};

/// Whether a path is a photo file Grainy opens (`engine.IMAGE_EXTENSIONS`: RAW and ordinary images).
pub fn is_photo_file(path: &Path) -> bool {
    path.is_file() && has_extension(path, &grainy_service::IMAGE_EXTENSIONS)
}

/// The photo files of `paths`: files as given (when they are photos), folders read with their subfolders
/// (`사진 폴더 가져오기 (하위 폴더 포함)`), each folder's files by name. Hidden files are skipped.
pub fn photo_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            folder_files(path, &mut out);
        } else if is_photo_file(path) {
            out.push(path.clone());
        }
    }
    out
}

fn folder_files(folder: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(|e| e.file_name().to_string_lossy().to_lowercase());
    let hidden = |p: &Path| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'));
    for entry in entries.iter().filter(|e| !hidden(&e.path())) {
        let path = entry.path();
        if is_photo_file(&path) {
            out.push(path);
        }
    }
    for entry in entries.iter().filter(|e| !hidden(&e.path())) {
        // Like the oracle's Path.rglob, do not recurse through directory links.
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            folder_files(&entry.path(), out);
        }
    }
}

/// The thumbnail cache key of a photo with its settings: a thumbnail is made again only when this changes.
pub fn thumb_key(path: &Path, settings: &Settings) -> u64 {
    let mut hasher = DefaultHasher::new();
    path.hash(&mut hasher);
    serde_json::to_string(&settings.0).unwrap_or_default().hash(&mut hasher);
    hasher.finish()
}

/// The session's photos. The open photo's [`EditState`] is [`Session::edit`]; the others keep theirs.
#[derive(Debug, Default)]
pub struct Session {
    photos: Vec<PhotoEntry>,
    /// Each photo's edit state; the open photo's lives in [`Session::edit`] while it is open.
    edits: Vec<EditState>,
    current: Option<usize>,
    /// The open photo's settings and undo (a scratch state when no photo is open).
    pub edit: EditState,
    next_id: u64,
    /// Selected photo ids (the open photo and Shift/Ctrl-clicked ones).
    selection: BTreeSet<u64>,
    /// Anchor photo id for Shift-click / Shift-arrow range selection.
    selection_anchor: Option<u64>,
    copied: Option<Settings>,
    /// The open photo's commits not yet handed to Auto Sync.
    commits: Vec<(u64, Commit)>,
}

impl Session {
    /// Writes `values` as one step each (the batch writer of the session).
    fn write_batch(&mut self, ids: &[u64], values: &[Settings], label: &str) -> Result<Vec<u64>, String> {
        let mut changed = Vec::new();
        for (&id, value) in ids.iter().zip(values) {
            let index = self.index_of(id).ok_or("photo not found")?;
            if crate::state::settings_eq(self.settings_of(index), value) {
                continue;
            }
            self.edit_of_mut(index).replace(value.clone(), label);
            changed.push(id);
        }
        Ok(changed)
    }

    /// Adds photos (a path already in the session is skipped); returns how many were added. The first photo of
    /// an empty session is opened.
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> usize {
        let mut added = 0;
        for path in paths {
            if self.photos.iter().any(|p| p.path == path) {
                continue;
            }
            self.next_id += 1;
            self.photos.push(PhotoEntry::new(self.next_id, path));
            self.edits.push(EditState::default());
            added += 1;
        }
        if self.current.is_none() && !self.photos.is_empty() {
            self.select(0);
        }
        added
    }

    /// The photos in order.
    pub fn photos(&self) -> &[PhotoEntry] {
        &self.photos
    }

    /// How many photos.
    pub fn len(&self) -> usize {
        self.photos.len()
    }

    /// Whether there are no photos.
    pub fn is_empty(&self) -> bool {
        self.photos.is_empty()
    }

    /// The open photo's index.
    pub fn current_index(&self) -> Option<usize> {
        self.current
    }

    /// The open photo.
    pub fn current(&self) -> Option<&PhotoEntry> {
        self.current.map(|i| &self.photos[i])
    }

    /// The open photo's id, 0 when none.
    pub fn current_id(&self) -> u64 {
        self.current().map_or(0, |p| p.id)
    }

    /// The index of a photo id.
    pub fn index_of(&self, id: u64) -> Option<usize> {
        self.photos.iter().position(|p| p.id == id)
    }

    /// Records a photo's original size.
    pub fn set_original(&mut self, id: u64, size: (u32, u32)) {
        if let Some(i) = self.index_of(id) {
            self.photos[i].original = Some(size);
        }
    }

    /// The settings of photo `index` (the live ones for the open photo).
    pub fn settings_of(&self, index: usize) -> &Settings {
        self.edit_of(index).settings()
    }

    /// The edit state of photo `index`.
    pub fn edit_of(&self, index: usize) -> &EditState {
        if self.current == Some(index) {
            &self.edit
        } else {
            &self.edits[index]
        }
    }

    fn edit_of_mut(&mut self, index: usize) -> &mut EditState {
        if self.current == Some(index) {
            &mut self.edit
        } else {
            &mut self.edits[index]
        }
    }

    /// Opens photo `index`: the open photo's pending change is committed and its state put back; the selection
    /// becomes that photo. Returns whether the open photo changed.
    pub fn select(&mut self, index: usize) -> bool {
        if index >= self.photos.len() {
            return false;
        }
        self.selection = BTreeSet::from([self.photos[index].id]);
        self.selection_anchor = Some(self.photos[index].id);
        if self.current == Some(index) {
            return false;
        }
        if let Some(current) = self.current {
            self.edit.commit("보정 변경");
            self.edits[current] = std::mem::take(&mut self.edit);
        }
        self.edit = std::mem::take(&mut self.edits[index]);
        self.current = Some(index);
        true
    }

    /// Opens the photo `delta` places away (arrow keys), stopping at the ends.
    pub fn step(&mut self, delta: isize) -> bool {
        let Some(current) = self.current else { return false };
        let target = current.saturating_add_signed(delta).min(self.photos.len().saturating_sub(1));
        self.select(target)
    }

    /// Ctrl/Cmd-click: adds or removes a photo from the selection (the open photo stays selected).
    pub fn toggle_selected(&mut self, index: usize) {
        let Some(photo) = self.photos.get(index) else { return };
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

    /// Shift-click / Shift-arrow: selects a contiguous range from the anchor (or open photo) to `index`.
    pub fn select_range(&mut self, index: usize, additive: bool) {
        if index >= self.photos.len() {
            return;
        }
        let anchor = self
            .selection_anchor
            .and_then(|id| self.photos.iter().position(|p| p.id == id))
            .or(self.current)
            .unwrap_or(index);
        if self.selection_anchor.is_none() {
            self.selection_anchor = self.photos.get(anchor).map(|p| p.id);
        }
        if !additive {
            self.selection.clear();
            let current_id = self.current_id();
            if current_id != 0 {
                self.selection.insert(current_id);
            }
        }
        let (lo, hi) = (anchor.min(index), anchor.max(index));
        for photo in &self.photos[lo..=hi] {
            self.selection.insert(photo.id);
        }
    }

    /// Selects every photo in the session.
    pub fn select_all(&mut self) {
        self.selection = self.photos.iter().map(|p| p.id).collect();
    }

    /// Clears multi-selection back to only the open photo.
    pub fn deselect_others(&mut self) {
        let current_id = self.current_id();
        self.selection = if current_id != 0 { BTreeSet::from([current_id]) } else { BTreeSet::new() };
        self.selection_anchor = (current_id != 0).then_some(current_id);
    }

    /// Whether a photo is selected.
    pub fn is_selected(&self, id: u64) -> bool {
        self.selection.contains(&id)
    }

    /// The selected photos' indices in order (at least the open photo).
    pub fn selected_indices(&self) -> Vec<usize> {
        let mut out: Vec<usize> =
            self.photos.iter().enumerate().filter(|(_, p)| self.selection.contains(&p.id)).map(|(i, _)| i).collect();
        if out.is_empty() {
            out.extend(self.current);
        }
        out
    }

    /// Port of `copy_edits`: copies the open photo's settings. False when no photo is open.
    pub fn copy(&mut self) -> bool {
        if self.current.is_none() {
            return false;
        }
        self.edit.commit("보정 변경");
        self.copied = Some(self.edit.settings().clone());
        true
    }

    /// Whether settings were copied.
    pub fn has_copied(&self) -> bool {
        self.copied.is_some()
    }

    /// Port of `paste_edits` with the default copy items (`manager.copy_keys`: everything but crop and rotation
    /// and `panels_off`), through `grainy_service::edits::copied_settings`: pastes into each photo of `targets`,
    /// one undo step "보정 붙여넣기" per photo. Returns how many photos changed, or the service's message.
    pub fn paste(&mut self, targets: &[usize]) -> Result<usize, String> {
        let Some(copied) = self.copied.clone() else { return Ok(0) };
        let mut changed = 0;
        for &index in targets {
            if index >= self.photos.len() {
                continue;
            }
            let pasted = pasted_settings(&copied, self.settings_of(index))?;
            self.edit_of_mut(index).replace(pasted, "보정 붙여넣기");
            changed += 1;
        }
        Ok(changed)
    }
}

/// `paste_edits` for one photo: `copied` over `target` with the default copy items
/// (`grainy_service::edits::copied_settings`, crop and rotation kept).
pub fn pasted_settings(copied: &Settings, target: &Settings) -> Result<Settings, String> {
    let mut keys = grainy_service::edits::copy_keys(None).map_err(|e| e.to_string())?;
    if crate::panels::metadata::paste_crop() {
        // 붙여넣기에 크롭·회전도 포함 (`copy_crop_changed`): `EDIT_GROUPS['크롭·회전']` joins the copy keys.
        keys.extend(["crop", "rotation", "straighten", "flip"].map(String::from));
    }
    let source = Value::Object(copied.0.clone());
    let info = Value::Object(Map::new());
    let settings = Value::Object(target.0.clone());
    let pasted =
        grainy_service::edits::copied_settings(&source, &settings, &info, &keys, &grainy_service::edits::Plain)
            .map_err(|e| e.to_string())?;
    match pasted {
        Value::Object(map) => Ok(normalized(&map)),
        _ => Err("paste".into()),
    }
}

impl Document for Session {
    fn kind(&self) -> Kind {
        Kind::Session
    }

    fn photos(&self) -> &[PhotoEntry] {
        &self.photos
    }

    fn current_index(&self) -> Option<usize> {
        self.current
    }

    fn select(&mut self, index: usize) -> bool {
        Session::select(self, index)
    }

    fn toggle_selected(&mut self, index: usize) {
        Session::toggle_selected(self, index);
    }

    fn select_range(&mut self, index: usize, additive: bool) {
        Session::select_range(self, index, additive);
    }

    fn select_all(&mut self) {
        Session::select_all(self);
    }

    fn deselect_others(&mut self) {
        Session::deselect_others(self);
    }

    fn is_selected(&self, id: u64) -> bool {
        Session::is_selected(self, id)
    }

    fn selected_indices(&self) -> Vec<usize> {
        Session::selected_indices(self)
    }

    fn edit(&self) -> &EditState {
        &self.edit
    }

    fn edit_mut(&mut self) -> &mut EditState {
        &mut self.edit
    }

    fn settings_of(&self, index: usize) -> Settings {
        Session::settings_of(self, index).clone()
    }

    fn version_of(&self, index: usize) -> u64 {
        self.edit_of(index).version()
    }

    fn set_original(&mut self, id: u64, size: (u32, u32)) {
        Session::set_original(self, id, size);
    }

    fn flush(&mut self) -> Result<(), String> {
        // Nothing is stored: the journal is only emptied.
        if let Some(id) = self.current().map(|p| p.id) {
            self.commits.extend(self.edit.take_commits().into_iter().map(|c| (id, c)));
        }
        self.edit.take_journal();
        for edit in &mut self.edits {
            edit.take_journal();
            edit.take_commits();
        }
        Ok(())
    }

    fn stored_settings_of(&self, id: u64) -> Result<Settings, String> {
        let index = self.index_of(id).ok_or("photo not found")?;
        Ok(self.settings_of(index).clone())
    }

    fn edit_photos(&mut self, ids: &[u64], settings: &[Settings], label: &str) -> Result<usize, String> {
        if ids.len() != settings.len() {
            return Err("일괄 보정 대상이 올바르지 않습니다.".into());
        }
        for (&id, value) in ids.iter().zip(settings) {
            let index = self.index_of(id).ok_or("photo not found")?;
            let value = crate::commands::sync::fitted(value, &Document::photo_info(self, id))?;
            self.edit_of_mut(index).replace(value, label);
        }
        Ok(ids.len())
    }

    fn edit_photos_batch(&mut self, ids: &[u64], settings: &[Settings], label: &str) -> Result<Vec<u64>, String> {
        let mut fitted = Vec::new();
        for (&id, value) in ids.iter().zip(settings) {
            fitted.push(crate::commands::sync::fitted(value, &Document::photo_info(self, id))?);
        }
        self.write_batch(ids, &fitted, label)
    }

    fn edit_photos_batch_own(&mut self, ids: &[u64], settings: &[Settings], label: &str) -> Result<Vec<u64>, String> {
        self.write_batch(ids, settings, label)
    }

    fn copied(&self) -> Option<Settings> {
        self.copied.clone()
    }

    fn take_commits(&mut self) -> Vec<(u64, Commit)> {
        std::mem::take(&mut self.commits)
    }

    fn history(&self) -> Vec<HistoryEntry> {
        self.edit
            .history()
            .iter()
            .map(|label| HistoryEntry { label: label.clone(), created: String::new(), settings: None })
            .collect()
    }

    fn copy(&mut self) -> bool {
        Session::copy(self)
    }

    fn has_copied(&self) -> bool {
        Session::has_copied(self)
    }

    fn paste(&mut self, targets: &[usize]) -> Result<usize, String> {
        Session::paste(self, targets)
    }

    fn set_label_field(&mut self, field: LabelField, value: Value) -> Result<(), String> {
        for index in self.selected_indices() {
            let photo = &mut self.photos[index];
            match field {
                LabelField::Rating => photo.rating = value.as_i64().unwrap_or(0),
                LabelField::Flag => photo.flag = value.as_i64().unwrap_or(0),
                LabelField::Label => photo.label = value.as_str().unwrap_or("").to_owned(),
            }
        }
        Ok(())
    }

    fn add_files(&mut self, files: Vec<PathBuf>, _roots: &[PathBuf]) -> Result<usize, String> {
        Ok(self.add(files))
    }

    fn remove_selected(&mut self) -> Result<usize, String> {
        let ids: BTreeSet<u64> = self.selected_indices().iter().map(|i| self.photos[*i].id).collect();
        if ids.is_empty() {
            return Ok(0);
        }
        if let Some(current) = self.current.take() {
            self.edits[current] = std::mem::take(&mut self.edit);
        }
        let first = self.photos.iter().position(|p| ids.contains(&p.id)).unwrap_or(0);
        let mut kept_photos = Vec::new();
        let mut kept_edits = Vec::new();
        for (photo, edit) in self.photos.drain(..).zip(self.edits.drain(..)) {
            if !ids.contains(&photo.id) {
                kept_photos.push(photo);
                kept_edits.push(edit);
            }
        }
        self.photos = kept_photos;
        self.edits = kept_edits;
        self.selection.clear();
        self.edit = EditState::default();
        if !self.photos.is_empty() {
            self.select(first.min(self.photos.len() - 1));
        }
        Ok(ids.len())
    }
}
