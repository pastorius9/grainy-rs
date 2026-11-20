//! The edit state of the open photo: its settings, the last committed settings and the undo/redo stacks.
//! Port of `set_setting`, `commit`, `undo`, `redo`, `apply_restored` and `DevelopPanels.switch_panel` of
//! luma/app.py and luma/develop_panels.py.
//!
//! The state itself never writes anything. Every commit and every undo/redo is also recorded in a journal
//! ([`Change`]); the document that owns the state ([`crate::document`]) drains the journal and stores the changes
//! (the catalogue writes `Catalog.edit` for a commit and `Catalog.set_settings` for an undo or redo, then the
//! `undo:{id}` preference), so code that edits never has to know where the photo lives.

use grainy_core::{normalized, Settings};
use grainy_i18n::tr_with;
use serde_json::{Map, Value};

use crate::panels;

/// How many undo and redo steps are stored with a photo (`save_undo` keeps the last 80 of each stack; in memory
/// the oracle keeps them all, and so does [`EditState`]).
pub const UNDO_LIMIT: usize = 80;

/// Python's `==` on JSON values: numbers compare by value (`0 == 0.0`), booleans equal 0 and 1.
pub fn py_eq(a: &Value, b: &Value) -> bool {
    fn number(v: &Value) -> Option<f64> {
        match v {
            Value::Number(n) => n.as_f64(),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }
    match (a, b) {
        (Value::Array(x), Value::Array(y)) => x.len() == y.len() && x.iter().zip(y).all(|(p, q)| py_eq(p, q)),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| py_eq(v, w)))
        }
        _ => match (number(a), number(b)) {
            (Some(x), Some(y)) => x == y,
            _ => a == b,
        },
    }
}

/// `dict == dict` for settings.
pub fn settings_eq(a: &Settings, b: &Settings) -> bool {
    a.0.len() == b.0.len() && a.0.iter().all(|(key, value)| b.0.get(key).is_some_and(|other| py_eq(value, other)))
}

/// Port of `panel_keys.changed_keys`: the keys whose values differ between two settings.
pub fn changed_keys(before: &Settings, after: &Settings) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    for (key, value) in after.0.iter() {
        if !py_eq(before.value(key), value) {
            keys.push(key.clone());
        }
    }
    for key in before.0.keys() {
        if !after.0.contains_key(key) && !keys.contains(key) {
            keys.push(key.clone());
        }
    }
    keys
}

/// What happened to the settings, for the document to store.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// `commit(label)`: the settings were committed as one undo step (the catalogue's `edit` with a history row).
    Committed { label: String, settings: Settings },
    /// `undo` / `redo` (`apply_restored`): the settings were replaced without a history row (`set_settings`).
    Restored { settings: Settings },
}

/// One commit as `LibraryManager.after_commit(previous)` sees it: the settings before and after.
#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub label: String,
    /// `last_saved` before the commit.
    pub previous: Settings,
    pub settings: Settings,
}

/// The settings of the photo being edited, with undo and redo.
#[derive(Debug, Clone)]
pub struct EditState {
    settings: Settings,
    last_saved: Settings,
    undo: Vec<Settings>,
    redo: Vec<Settings>,
    history: Vec<String>,
    version: u64,
    journal: Vec<Change>,
    commits: Vec<Commit>,
}

impl Default for EditState {
    fn default() -> Self {
        Self::new(normalized(&Map::new()))
    }
}

impl EditState {
    /// A photo opened with these settings (already normalized).
    pub fn new(settings: Settings) -> Self {
        Self::with_stacks(settings, Vec::new(), Vec::new())
    }

    /// A photo opened with its stored undo and redo stacks (`activate`: `undo:{id}`, each entry normalized).
    pub fn with_stacks(settings: Settings, undo: Vec<Settings>, redo: Vec<Settings>) -> Self {
        Self {
            last_saved: settings.clone(),
            settings,
            undo: undo.iter().map(|s| normalized(&s.0)).collect(),
            redo: redo.iter().map(|s| normalized(&s.0)).collect(),
            history: Vec::new(),
            version: 0,
            journal: Vec::new(),
            commits: Vec::new(),
        }
    }

    /// The current settings.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The last committed settings (`last_saved`).
    pub fn last_saved(&self) -> &Settings {
        &self.last_saved
    }

    /// Changes with every edit: the renderer runs when it changes.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Port of `set_setting`: sets a value without committing, switches on the panel that owns it, and moves a
    /// roll-off photo to the current process when Highlights or Shadows move.
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        self.settings.set(key, value);
        if (key == "highlights" || key == "shadows") && self.settings.i64("tone_version") == 2 {
            self.settings.set("tone_version", 3);
        }
        panels::switch_on(&mut self.settings, &[key]);
        self.version += 1;
    }

    /// Replaces the live settings without committing and without switching panels (an automatic result that has
    /// already switched its panels on). The caller commits.
    pub fn set_all(&mut self, settings: Settings) {
        self.settings = settings;
        self.version += 1;
    }

    /// Whether there are changes since the last commit.
    pub fn dirty(&self) -> bool {
        !settings_eq(&self.settings, &self.last_saved)
    }

    /// Port of `commit(label)`: one undo step for everything changed since the last commit. Returns whether
    /// there was anything to commit.
    pub fn commit(&mut self, label: &str) -> bool {
        if !self.dirty() {
            return false;
        }
        self.undo.push(self.last_saved.clone());
        self.redo.clear();
        self.commits.push(Commit {
            label: label.to_owned(),
            previous: self.last_saved.clone(),
            settings: self.settings.clone(),
        });
        self.last_saved = self.settings.clone();
        self.history.insert(0, label.to_owned());
        self.journal.push(Change::Committed { label: label.to_owned(), settings: self.settings.clone() });
        true
    }

    /// Port of `undo`.
    pub fn undo(&mut self) -> bool {
        self.commit("보정 변경");
        let Some(previous) = self.undo.pop() else { return false };
        self.redo.push(self.settings.clone());
        self.restore(previous);
        true
    }

    /// Port of `redo`.
    pub fn redo(&mut self) -> bool {
        self.commit("보정 변경");
        let Some(next) = self.redo.pop() else { return false };
        self.undo.push(self.settings.clone());
        self.restore(next);
        true
    }

    /// Port of `apply_restored`.
    fn restore(&mut self, settings: Settings) {
        self.settings = normalized(&settings.0);
        self.last_saved = self.settings.clone();
        self.version += 1;
        self.journal.push(Change::Restored { settings: self.settings.clone() });
    }

    /// Steps available: (undo, redo).
    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }

    /// The stacks as `save_undo` stores them: the last [`UNDO_LIMIT`] of each.
    pub fn stored_stacks(&self) -> (&[Settings], &[Settings]) {
        let tail = |v: &[Settings]| v.len().saturating_sub(UNDO_LIMIT);
        (&self.undo[tail(&self.undo)..], &self.redo[tail(&self.redo)..])
    }

    /// The labels committed in this session, newest first (the session's History list).
    pub fn history(&self) -> &[String] {
        &self.history
    }

    pub(crate) fn journal(&self) -> &[Change] {
        &self.journal
    }

    /// Takes the changes recorded since the last call (the document stores them).
    pub fn take_journal(&mut self) -> Vec<Change> {
        std::mem::take(&mut self.journal)
    }

    /// Takes the commits made since the last call (Auto Sync reads what each one changed).
    pub fn take_commits(&mut self) -> Vec<Commit> {
        std::mem::take(&mut self.commits)
    }

    /// Whether changes wait to be stored.
    pub fn has_journal(&self) -> bool {
        !self.journal.is_empty()
    }

    /// Port of `DevelopPanels.switch_panel`: the header switch, one undo step labelled "{title} 켜기/끄기"; the
    /// values stay.
    pub fn switch_panel(&mut self, panel: &str, on: bool) {
        if panels::is_on(&self.settings, panel) == on {
            return;
        }
        self.commit("보정 변경");
        match panels::switch_key(panel) {
            Some(key) => self.settings.set(key, on),
            None => {
                let list = panels::panels_off_with(&self.settings, panel, on);
                self.settings.set("panels_off", list);
            }
        }
        self.version += 1;
        let title = grainy_i18n::tr(panels::title(panel));
        let label = if on { tr_with!("{0} 켜기", title) } else { tr_with!("{0} 끄기", title) };
        self.commit(&label);
    }

    /// Replaces every value (a paste, a preset, a history entry): one undo step labelled `label`.
    pub fn replace(&mut self, settings: Settings, label: &str) {
        self.commit("보정 변경");
        self.settings = normalized(&settings.0);
        self.version += 1;
        self.commit(label);
    }

    /// Port of `reset_edits` without its confirmation: every value back to its default, one undo step (the
    /// oracle commits it with the default label).
    pub fn reset(&mut self) {
        self.commit("보정 변경");
        self.settings = normalized(&Map::new());
        self.version += 1;
        self.commit("보정 변경");
    }

    /// The keys whose values differ from the defaults, with their values (the debug overlay).
    pub fn changed(&self) -> Map<String, Value> {
        let defaults = grainy_core::defaults();
        self.settings
            .0
            .iter()
            .filter(|(key, value)| !py_eq(value, defaults.value(key)))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}
