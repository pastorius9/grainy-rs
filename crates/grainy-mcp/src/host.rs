//! What the tools need from the Grainy window (the oracle's `MainWindow` as `mcp_tools` and `mcp_service` use it),
//! and how they reach the thread that owns it (the oracle's `GuiCall`).
//!
//! The application implements [`Host`] on its window state and [`Gui`] on its event loop
//! ([`crate::gui::GuiQueue`] does the waiting rules of `GuiCall` for it); [`crate::headless`] implements both over
//! a catalogue for tests and a windowless mode.

use std::any::Any;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use grainy_catalog::Catalog;
use serde_json::Value;

/// A value computed off the GUI thread, handed to the code that finishes it on the GUI thread.
pub type Payload = Box<dyn Any + Send>;
/// Work for the window's MCP worker pool (`w.spawn(work, ...)`).
pub type Work = Box<dyn FnOnce() -> Payload + Send>;
/// What runs on the GUI thread with the result of a [`Work`] (`w.spawn(..., done, failed)`).
pub type Done = Box<dyn FnOnce(&mut dyn Host, Payload) + Send>;
/// Work on the GUI thread (a `GuiCall`, a `QTimer.singleShot`, one tick of a `QTimer`).
pub type GuiTask = Box<dyn FnOnce(&mut dyn Host) + Send>;

/// Why a call into the GUI thread did not give a result (`mcp_service.GuiCall.call`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    /// `Busy`: the call never started (closed, or not started within `QUEUE_SECONDS`); the text completes
    /// "Grainy is busy: ...".
    Busy(String),
    /// `Unfinished`: it started but did not finish before the deadline plus `GRACE_SECONDS`.
    Unfinished,
}

/// The oracle's `GuiCall`: runs a task on the thread that owns the window. A new one per server start, so a request
/// left over from a stopped server finds it closed (Busy) and never writes through a later server.
pub trait Gui: Send + Sync {
    /// Run `task` on the GUI thread and return once it ran (`GuiCall.call(fn, deadline)`).
    fn call(&self, task: GuiTask, deadline: Instant) -> Result<(), CallError>;
}

/// `on_done(result)` of an export, on the GUI thread.
pub type OnDone = Box<dyn FnOnce(&mut dyn Host, Value) + Send>;
/// `on_error(error)` of an export, on the GUI thread.
pub type OnError = Box<dyn FnOnce(&mut dyn Host, String) + Send>;

/// The arguments of the window's export (`MainWindow.start_export(ids, folder, format, quality, longest,
/// color_space, keep_metadata, naming, True, True, remember=False, on_done, on_error, cancel, progress,
/// protocol=True)`).
pub struct ExportStart {
    /// The photos, in order.
    pub ids: Vec<i64>,
    /// The destination folder (exists).
    pub folder: std::path::PathBuf,
    /// Format, quality, size, colour space, metadata and naming.
    pub options: grainy_service::export::ExportOptions,
    /// The job's cancel flag (the stop button, or the connection going off).
    pub cancel: Arc<AtomicBool>,
    /// Called with the number of photos done so far (any thread).
    pub progress: Arc<dyn Fn(usize) + Send + Sync>,
    /// `on_done(result)` on the GUI thread: `{outputs, failures, skipped[, completion_error]}`.
    pub on_done: OnDone,
    /// `on_error(error)` on the GUI thread, with `photo_service.english_error` of the failure.
    pub on_error: OnError,
}

/// The Grainy window as the AI connection uses it. Every method is called on the GUI thread (inside a
/// [`Gui::call`] task, a [`Done`] or a [`GuiTask`] given to [`Host::later`]).
pub trait Host {
    // --- readiness (mcp_service.ready) -------------------------------------------------------------------
    /// `w.closing`: the window is closing or being rebuilt.
    fn closing(&self) -> bool;
    /// `mcp_service.nested()`: a modal dialog, a popup menu or a nested event loop is open.
    fn modal_open(&self) -> bool;
    /// `QThread.currentThread().loopLevel() != 1`: the GUI thread is inside a nested event loop (safe_commit).
    fn nested_loop(&self) -> bool;
    /// `w.command_running`: a photo batch of the menus is running.
    fn command_running(&self) -> bool;
    /// `w.maintenance_running`: a catalogue task is running.
    fn maintenance_running(&self) -> bool;
    /// `w.import_busy or w.import_scans`.
    fn importing(&self) -> bool;
    /// `w.import_scans`: folders are still being scanned for an import.
    fn import_scanning(&self) -> bool;
    /// `w.export_running`.
    fn export_running(&self) -> bool;
    /// `w.interacting()`: the person is dragging a slider or a point on the open photo.
    fn interacting(&self) -> bool;
    /// `w.studio.pending_stroke is not None or w.studio.healing_strokes`.
    fn unfinished_marks(&self) -> bool;
    /// `w.view.crop_mode or w.view.sample_mode`.
    fn tool_open(&self) -> bool;
    /// `w.library_loading`: the photo list is being filled.
    fn library_loading(&self) -> bool;

    // --- the catalogue and the open photo -----------------------------------------------------------------
    /// `w.catalog`: the window's (writing) catalogue connection.
    fn catalog(&self) -> &Catalog;
    /// `w.current_id`: the photo open in the editor.
    fn current_id(&self) -> Option<i64>;
    /// `w.settings != w.last_saved`: the open photo has a change not yet written.
    fn unsaved(&self) -> bool;
    /// `deepcopy(w.settings)`: the editor's settings of the open photo (unsaved changes included).
    fn editor_settings(&self) -> Option<Value>;
    /// `w.commit()`: write the open photo's pending change (history and undo state as the editor does).
    fn commit(&mut self) -> grainy_service::Result<()>;
    /// `w.save_keywords()`: write the keywords field of the open photo.
    fn save_keywords(&mut self) -> grainy_service::Result<()>;
    /// `w.selected_ids()`: the selected photos (or the open one when it is listed), `[]` while loading.
    fn selected_ids(&self) -> Vec<i64>;
    /// `w.all_presets`: Grainy's presets by name, built in first, then the saved ones (normalized settings).
    fn presets(&self) -> grainy_service::Result<Vec<(String, Value)>>;
    /// `id in w.photo_model.positions`: whether a photo is in the current list view.
    fn listed(&self, id: i64) -> bool;

    // --- showing what changed ------------------------------------------------------------------------------
    /// `w.refresh_command_edits(ids, label)`: thumbnails and, when the open photo is among them, the editor.
    fn refresh_command_edits(&mut self, ids: &[i64], label: &str);
    /// `w.refresh_lists()`: the photo lists (ratings, labels, new photos).
    fn refresh_lists(&mut self);
    /// `w.load_controls()`: the editor's controls from the open photo's settings.
    fn load_controls(&mut self);
    /// `w.activate(id)`: open a photo in the editor (commits the previous one).
    fn activate(&mut self, id: i64) -> grainy_service::Result<()>;
    /// `w.set_mode(0 | 1)`: the editor (`true`) or the library view.
    fn set_mode(&mut self, edit: bool);
    /// `w.active_list().restore_selection(ids, current)` with the selection signals blocked.
    fn restore_selection(&mut self, ids: &[i64], current: i64);

    // --- work off the GUI thread ---------------------------------------------------------------------------
    /// `w.spawn(work, done, failed, pool=w.mcp.pool)`: run `work` in the window's MCP pool (one worker), then
    /// `done` on the GUI thread. Dropped silently when the window is closing (as `job_finished`).
    fn spawn(&mut self, work: Work, done: Done);
    /// `QTimer.singleShot(ms, w, task)`: run `task` on the GUI thread after `ms` milliseconds.
    fn later(&mut self, ms: u64, task: GuiTask);
    /// `w.start_export(...)`: the window's export in its worker pool; `Ok(false)` when an export is running
    /// already (nothing started).
    fn start_export(&mut self, export: ExportStart) -> grainy_service::Result<bool>;

    // --- the import queue ----------------------------------------------------------------------------------
    /// `w.import_quiet |= keys; w.queue_import_files(files)`: import these files with the window's queue
    /// (duplicates skipped, failures reported by the job rather than a message box). `keys` are their
    /// `folders.path_key`s.
    fn queue_import_files(&mut self, files: &[String], keys: &[String]);
    /// How many of `keys` are still waiting in the import queue (`keys & w.import_pending_paths`).
    fn import_waiting(&self, keys: &[String]) -> usize;
    /// `w.import_quiet -= keys - w.import_pending_paths`: failures of these files are the person's to see again.
    fn release_import_quiet(&mut self, keys: &[String]);
    /// `w.import_completion_errors.pop(key)` for each key that has one: imports that could not be saved.
    fn take_import_completion_errors(&mut self, keys: &[String]) -> Vec<String>;
    /// `w.cancel_import()`: the stop button of an import job stops the window's import queue.
    fn cancel_import(&mut self);
}
