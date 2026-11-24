//! A [`Host`] without a window: the tools over a catalogue, for tests and a windowless mode (`grainy-cli mcp`).
//!
//! It behaves as the fake window the goldens are recorded with (`FakeWindow` in `tools/golden/gen_mcp.py`), which
//! keeps to what `MainWindow` does for the AI connection: the open photo's editor state (settings, last saved
//! settings, undo stack written to `undo:<id>` as the editor's commit does), the selection, the busy flags, the
//! window's export (`photo_service.export_records` with the engine) and an import queue (each file that loads is
//! added to the catalogue). UI calls are recorded in [`HeadlessHost::events`].
//!
//! Work and timers run as [`Mode`] says: `Deferred` keeps them in a queue that [`Headless::run_pending`] runs in
//! order (deterministic job states for tests); `Inline` runs spawned work at once (timers still wait in the queue);
//! `Threaded` runs work on threads and timers after their delay, as an application would.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use grainy_catalog::Catalog;
use serde_json::{json, Map, Value};

use crate::host::{CallError, Done, ExportStart, Gui, GuiTask, Host, Payload, Work};
use crate::service::CLOSED;
use crate::util::{eq, lock};

/// How a [`Headless`] runs spawned work and timers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Work and timers wait in a queue until [`Headless::run_pending`].
    Deferred,
    /// Spawned work runs at once (within the call); timers wait in the queue.
    Inline,
    /// Work runs on threads, timers after their delay.
    Threaded,
}

type OwnDone = Box<dyn FnOnce(&mut HeadlessHost, Payload) + Send>;

enum Queued {
    Spawn(Work, Done),
    Own(Work, OwnDone),
    Once(GuiTask),
    Import(Vec<(String, String)>),
}

type Queue = Arc<Mutex<VecDeque<Queued>>>;

/// Port of Grainy's built-in presets (`engine.PRESETS`), in order.
pub fn built_in_presets() -> Vec<(String, Value)> {
    let raw = [
        ("원본의 색", json!({})),
        (
            "Soft daylight",
            json!({"exposure": 0.15, "contrast": -12, "highlights": -22, "shadows": 18, "temperature": 4, "saturation": -6}),
        ),
        (
            "Amber film",
            json!({"temperature": 16, "tint": 4, "contrast": 12, "highlights": -28, "shadows": 12, "saturation": -14,
                   "grain": 12, "vignette": 12}),
        ),
        ("Silver / B&W", json!({"monochrome": true, "contrast": 24, "highlights": -15, "shadows": 10, "grain": 16})),
        (
            "Crisp landscape",
            json!({"contrast": 15, "vibrance": 22, "highlights": -24, "shadows": 22, "clarity": 22, "sharpen": 25}),
        ),
        ("Quiet blue", json!({"temperature": -13, "saturation": -16, "contrast": -6, "shadows": 20, "blacks": 8})),
    ];
    let tone = grainy_core::defaults().0.get("tone_version").cloned().unwrap_or(Value::Null);
    raw.into_iter()
        .map(|(name, values)| {
            let mut map = Map::new();
            map.insert("tone_version".into(), tone.clone());
            if let Value::Object(v) = values {
                map.extend(v);
            }
            let value = Value::Object(map);
            (name.to_owned(), grainy_service::normalized(&value).unwrap_or(value))
        })
        .collect()
}

/// The window state of a [`Headless`] (the oracle's `MainWindow` as the AI connection sees it).
pub struct HeadlessHost {
    catalog: Catalog,
    /// `w.closing`.
    pub closing: bool,
    /// `w.command_running`.
    pub command_running: bool,
    /// `w.maintenance_running`.
    pub maintenance_running: bool,
    /// `w.import_busy`.
    pub import_busy: bool,
    /// `w.import_scans` (non-empty).
    pub import_scans: bool,
    /// `w.export_running`.
    pub export_running: bool,
    /// `w.library_loading`.
    pub library_loading: bool,
    /// The selected photos.
    pub selection: Vec<i64>,
    /// The photos of the list view (`None`: every photo).
    pub listed: Option<Vec<i64>>,
    /// The person is dragging a control (`w.interacting()`).
    pub adjusting: bool,
    /// A dialog or menu is open (`mcp_service.nested()`).
    pub modal: bool,
    /// The event loop level (1 normally).
    pub loop_level: i32,
    /// A brush stroke is pending.
    pub pending_stroke: bool,
    /// Healing marks are pending.
    pub healing_strokes: bool,
    /// The crop tool is open.
    pub crop_mode: bool,
    /// A colour picker is open.
    pub sample_mode: bool,
    /// The editor (`true`) or the library view.
    pub edit_mode: bool,
    /// UI calls, as `[name, ...arguments]`.
    pub events: Vec<Value>,
    current: Option<i64>,
    settings: Option<Value>,
    last_saved: Option<Value>,
    undo: Vec<Value>,
    redo: Vec<Value>,
    import_quiet: HashSet<String>,
    import_pending: HashSet<String>,
    import_completion_errors: Map<String, Value>,
    mode: Mode,
    queue: Queue,
    me: Weak<Mutex<HeadlessHost>>,
}

impl HeadlessHost {
    /// The catalogue.
    pub fn catalog_ref(&self) -> &Catalog {
        &self.catalog
    }

    /// Change how work and timers run.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// The editor's settings of the open photo (`w.settings[key] = value`): an unsaved change.
    pub fn edit(&mut self, key: &str, value: Value) {
        if let Some(Value::Object(map)) = self.settings.as_mut() {
            map.insert(key.into(), value);
        }
    }

    /// Select photos.
    pub fn select(&mut self, ids: &[i64]) {
        self.selection = ids.to_vec();
    }

    fn load_state(&mut self) -> grainy_service::Result<()> {
        let Some(current) = self.current else { return Ok(()) };
        let settings = self.catalog.photo(current)?.map(|p| p["settings"].clone());
        self.last_saved = settings.clone();
        self.settings = settings;
        let state = self.catalog.preference(&format!("undo:{current}"), json!({}))?;
        let list = |key: &str| -> Vec<Value> {
            state
                .get(key)
                .and_then(Value::as_array)
                .map(|a| a.iter().map(|s| grainy_service::normalized(s).unwrap_or_else(|_| s.clone())).collect())
                .unwrap_or_default()
        };
        self.undo = list("undo");
        self.redo = list("redo");
        Ok(())
    }

    fn photo_ids(&self) -> Vec<i64> {
        self.catalog
            .rows("SELECT id FROM photos", [])
            .map(|rows| rows.iter().filter_map(|r| r["id"].as_i64()).collect())
            .unwrap_or_default()
    }

    fn queue_item(&mut self, item: Queued) {
        lock(&self.queue).push_back(item);
    }

    fn spawn_own(&mut self, work: Work, done: OwnDone) {
        match self.mode {
            Mode::Inline => {
                let value = work();
                if !self.closing {
                    done(self, value);
                }
            }
            Mode::Deferred => self.queue_item(Queued::Own(work, done)),
            Mode::Threaded => {
                let me = self.me.clone();
                std::thread::spawn(move || {
                    let value = work();
                    if let Some(host) = me.upgrade() {
                        let mut host = lock(&host);
                        if !host.closing {
                            done(&mut host, value);
                        }
                    }
                });
            }
        }
    }

    fn import_files(&mut self, files: Vec<(String, String)>) {
        let defaults = grainy_core::Settings(
            grainy_service::normalized(&json!({})).ok().and_then(|v| v.as_object().cloned()).unwrap_or_default(),
        );
        for (file, key) in files {
            if grainy_engine::files::load_photo(&file, Some(64), &defaults).is_ok() {
                let _ = self.catalog.add(&file, None, None, true);
            }
            self.import_pending.remove(&key);
        }
    }
}

impl Host for HeadlessHost {
    fn closing(&self) -> bool {
        self.closing
    }
    fn modal_open(&self) -> bool {
        self.modal
    }
    fn nested_loop(&self) -> bool {
        self.loop_level != 1
    }
    fn command_running(&self) -> bool {
        self.command_running
    }
    fn maintenance_running(&self) -> bool {
        self.maintenance_running
    }
    fn importing(&self) -> bool {
        self.import_busy || self.import_scans
    }
    fn import_scanning(&self) -> bool {
        self.import_scans
    }
    fn export_running(&self) -> bool {
        self.export_running
    }
    fn interacting(&self) -> bool {
        self.adjusting
    }
    fn unfinished_marks(&self) -> bool {
        self.pending_stroke || self.healing_strokes
    }
    fn tool_open(&self) -> bool {
        self.crop_mode || self.sample_mode
    }
    fn library_loading(&self) -> bool {
        self.library_loading
    }
    fn catalog(&self) -> &Catalog {
        &self.catalog
    }
    fn current_id(&self) -> Option<i64> {
        self.current
    }
    fn unsaved(&self) -> bool {
        match (&self.settings, &self.last_saved) {
            (Some(a), Some(b)) => !eq(a, b),
            (None, None) => false,
            _ => true,
        }
    }
    fn editor_settings(&self) -> Option<Value> {
        self.settings.clone()
    }
    fn commit(&mut self) -> grainy_service::Result<()> {
        let label = "보정 변경";
        let Some(current) = self.current.filter(|c| *c != 0) else { return Ok(()) };
        if !self.unsaved() {
            return Ok(());
        }
        self.undo.push(self.last_saved.clone().unwrap_or(Value::Null));
        self.redo.clear();
        let settings = self.settings.clone().unwrap_or_else(|| json!({}));
        self.catalog.edit(current, &grainy_core::Settings(settings.as_object().cloned().unwrap_or_default()), label)?;
        self.last_saved = Some(settings);
        let tail = |list: &[Value]| Value::Array(list[list.len().saturating_sub(80)..].to_vec());
        self.catalog.save_preference(
            &format!("undo:{current}"),
            &json!({"undo": tail(&self.undo), "redo": tail(&self.redo)}),
        )?;
        self.events.push(json!(["commit", current, label]));
        Ok(())
    }
    fn save_keywords(&mut self) -> grainy_service::Result<()> {
        Ok(())
    }
    fn selected_ids(&self) -> Vec<i64> {
        if self.library_loading {
            return Vec::new();
        }
        if !self.selection.is_empty() {
            return self.selection.clone();
        }
        match self.current {
            Some(c) if self.listed(c) => vec![c],
            _ => Vec::new(),
        }
    }
    fn presets(&self) -> grainy_service::Result<Vec<(String, Value)>> {
        let mut all = built_in_presets();
        if let Value::Object(saved) = self.catalog.presets()? {
            for (name, value) in saved {
                match all.iter_mut().find(|(n, _)| *n == name) {
                    Some(slot) => slot.1 = value,
                    None => all.push((name, value)),
                }
            }
        }
        Ok(all)
    }
    fn listed(&self, id: i64) -> bool {
        match &self.listed {
            Some(list) => list.contains(&id),
            None => self.photo_ids().contains(&id),
        }
    }
    fn refresh_command_edits(&mut self, ids: &[i64], label: &str) {
        self.events.push(json!(["refresh_command_edits", ids, label]));
        if self.current.is_some_and(|c| ids.contains(&c)) {
            let _ = self.load_state();
        }
    }
    fn refresh_lists(&mut self) {
        self.events.push(json!(["refresh_lists"]));
    }
    fn load_controls(&mut self) {
        self.events.push(json!(["load_controls"]));
    }
    fn activate(&mut self, id: i64) -> grainy_service::Result<()> {
        if Some(id) == self.current {
            return Ok(());
        }
        self.commit()?;
        self.save_keywords()?;
        self.current = Some(id);
        self.load_state()?;
        self.events.push(json!(["activate", id]));
        Ok(())
    }
    fn set_mode(&mut self, edit: bool) {
        self.edit_mode = edit;
        self.events.push(json!(["set_mode", if edit { 0 } else { 1 }]));
    }
    fn restore_selection(&mut self, ids: &[i64], current: i64) {
        self.selection = ids.to_vec();
        self.events.push(json!(["restore_selection", ids, current]));
    }
    fn spawn(&mut self, work: Work, done: Done) {
        match self.mode {
            Mode::Deferred => self.queue_item(Queued::Spawn(work, done)),
            _ => self.spawn_own(work, Box::new(move |host: &mut HeadlessHost, value| done(host, value))),
        }
    }
    fn later(&mut self, ms: u64, task: GuiTask) {
        if self.mode == Mode::Threaded {
            let me = self.me.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(ms));
                if let Some(host) = me.upgrade() {
                    task(&mut *lock(&host));
                }
            });
        } else {
            self.queue_item(Queued::Once(task));
        }
    }
    fn start_export(&mut self, export: ExportStart) -> grainy_service::Result<bool> {
        if self.export_running || export.records.is_empty() {
            return Ok(false);
        }
        self.commit()?;
        let records = export.records;
        self.export_running = true;
        self.events.push(json!(["export_started", records.len()]));
        let ExportStart { folder, options, cancel, progress, on_done, on_error, .. } = export;
        let work: Work = Box::new(move || {
            let workers = grainy_library::quick_export::rs_export_concurrency_here(&records).unwrap_or(1).max(1);
            let report = move |n: usize| progress(n);
            let result = grainy_service::export::export_records(
                &records,
                &folder,
                &options,
                Some(&cancel),
                Some(&report),
                &crate::engine::EngineExporter,
                true,
                workers,
            );
            Box::new(result.map_err(|e| crate::util::english(&e))) as Payload
        });
        let done: OwnDone = Box::new(move |host, payload| {
            host.export_running = false;
            let result =
                *payload.downcast::<Result<Value, String>>().unwrap_or_else(|_| Box::new(Err("internal".into())));
            match result {
                Ok(mut result) => {
                    let outputs = result["outputs"].as_array().cloned().unwrap_or_default();
                    if !outputs.is_empty() {
                        let items: Vec<Value> = outputs
                            .iter()
                            .map(|o| json!({"path": o["path"], "photo_id": o["photo_id"], "edit_signature": o["edit_signature"]}))
                            .collect();
                        if grainy_catalog::workflow::record_exports(&host.catalog, &items).is_err() {
                            result["completion_error"] =
                                "could not save export history; files already written stay".into();
                        }
                    }
                    on_done(host, result);
                }
                Err(last) => on_error(host, last),
            }
        });
        self.spawn_own(work, done);
        Ok(true)
    }
    fn queue_import_files(&mut self, files: &[String], keys: &[String]) {
        self.import_quiet.extend(keys.iter().cloned());
        let platform = grainy_catalog::folders::Platform::native();
        let pairs: Vec<(String, String)> = files.iter().map(|f| (f.clone(), platform.path_key(f))).collect();
        self.import_pending.extend(pairs.iter().map(|(_, k)| k.clone()));
        self.events.push(json!(["queue_import_files", files.len()]));
        if self.mode == Mode::Threaded {
            let me = self.me.clone();
            std::thread::spawn(move || {
                if let Some(host) = me.upgrade() {
                    lock(&host).import_files(pairs);
                }
            });
        } else {
            self.queue_item(Queued::Import(pairs));
        }
    }
    fn import_waiting(&self, keys: &[String]) -> usize {
        keys.iter().filter(|k| self.import_pending.contains(*k)).count()
    }
    fn release_import_quiet(&mut self, keys: &[String]) {
        for key in keys {
            if !self.import_pending.contains(key) {
                self.import_quiet.remove(key);
            }
        }
    }
    fn take_import_completion_errors(&mut self, keys: &[String]) -> Vec<String> {
        keys.iter().filter_map(|k| self.import_completion_errors.shift_remove(k)).map(|v| v.to_string()).collect()
    }
    fn cancel_import(&mut self) {
        self.events.push(json!(["cancel_import"]));
    }
}

/// A [`HeadlessHost`] behind its lock, and the [`Gui`] that reaches it.
pub struct Headless {
    host: Arc<Mutex<HeadlessHost>>,
    queue: Queue,
    closed: Mutex<bool>,
}

impl Headless {
    /// A window over `catalog`, running work as `mode` says.
    pub fn new(catalog: Catalog, mode: Mode) -> Arc<Self> {
        let queue: Queue = Arc::new(Mutex::new(VecDeque::new()));
        let host = Arc::new_cyclic(|me| {
            Mutex::new(HeadlessHost {
                catalog,
                closing: false,
                command_running: false,
                maintenance_running: false,
                import_busy: false,
                import_scans: false,
                export_running: false,
                library_loading: false,
                selection: Vec::new(),
                listed: None,
                adjusting: false,
                modal: false,
                loop_level: 1,
                pending_stroke: false,
                healing_strokes: false,
                crop_mode: false,
                sample_mode: false,
                edit_mode: true,
                events: Vec::new(),
                current: None,
                settings: None,
                last_saved: None,
                undo: Vec::new(),
                redo: Vec::new(),
                import_quiet: HashSet::new(),
                import_pending: HashSet::new(),
                import_completion_errors: Map::new(),
                mode,
                queue: queue.clone(),
                me: me.clone(),
            })
        });
        Arc::new(Self { host, queue, closed: Mutex::new(false) })
    }

    /// Work on the window state (the test's hand on the window).
    pub fn with<R>(&self, f: impl FnOnce(&mut HeadlessHost) -> R) -> R {
        f(&mut lock(&self.host))
    }

    /// Close (or reopen) the GUI call path: calls get Busy as from a stopped server's GuiCall.
    pub fn set_closed(&self, closed: bool) {
        *lock(&self.closed) = closed;
    }

    /// Queued work and timers.
    pub fn pending(&self) -> usize {
        lock(&self.queue).len()
    }

    /// Run queued work and timers in order: at most `limit` items, or until the queue is empty (at most 100).
    /// Returns how many ran.
    pub fn run_pending(&self, limit: Option<usize>) -> usize {
        let mut count = 0;
        while count < limit.unwrap_or(100) {
            let Some(item) = lock(&self.queue).pop_front() else { break };
            count += 1;
            match item {
                Queued::Spawn(work, done) => {
                    let value = work();
                    let mut host = lock(&self.host);
                    if !host.closing {
                        done(&mut *host, value);
                    }
                }
                Queued::Own(work, done) => {
                    let value = work();
                    let mut host = lock(&self.host);
                    if !host.closing {
                        done(&mut host, value);
                    }
                }
                Queued::Once(task) => task(&mut *lock(&self.host)),
                Queued::Import(files) => lock(&self.host).import_files(files),
            }
        }
        count
    }
}

impl Gui for Headless {
    fn call(&self, task: GuiTask, deadline: Instant) -> Result<(), CallError> {
        if *lock(&self.closed) {
            return Err(CallError::Busy(CLOSED.into()));
        }
        let _ = deadline;
        task(&mut *lock(&self.host));
        Ok(())
    }
}
