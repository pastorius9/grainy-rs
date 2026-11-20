//! The window side of every AI tool: grainy-mcp's [`Host`] implemented on the application context (port of what
//! luma/mcp_service.py and mcp_tools.py use of `MainWindow`: readiness, the open photo and its commit, the
//! selection, refreshes after a batch, the MCP worker pool, timers, the window's export and import queue).
//!
//! Calls arrive through the connection's [`grainy_mcp::gui::GuiQueue`], which [`pump`] runs on the window's thread
//! every frame. Work the tools hand over runs on the job service ([`crate::jobs::Jobs`]); its `done` runs with a
//! new [`AppHost`] when it finishes (dropped when the window closed or another library opened meanwhile).

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use grainy_catalog::Catalog;
use grainy_mcp::host::{Done, ExportStart, GuiTask, Host, Payload, Work};
use serde_json::{json, Value};

use crate::context::{AppContext, Mode};
use crate::panels::Tool;
use crate::text::tr;

/// Job kinds of the menus' photo batches (`command_running`).
const COMMAND_KINDS: [&str; 6] = ["workflow", "auto", "auto_level", "crop_border", "preset", "metadata_refresh"];
/// The MCP worker pool's job kind (`McpWindowState.pool`).
pub const POOL_KIND: &str = "mcp";
/// The AI import queue's job kind.
pub const IMPORT_KIND: &str = "mcp_import";

/// What the window keeps for the AI connection's calls between frames.
#[derive(Default)]
pub struct HostState {
    /// `w.closing`.
    pub closing: bool,
    /// Timers (`QTimer.singleShot`): when, and what.
    timers: Vec<(Instant, GuiTask)>,
    /// `import_pending_paths` of the AI imports (path keys).
    pending: BTreeSet<String>,
    /// `import_quiet`: failures the job reports instead of a message box.
    quiet: BTreeSet<String>,
    /// `import_completion_errors`.
    completion_errors: BTreeMap<String, String>,
}

/// The application context as the AI tools see the window. Built only while a library open for writing is shown
/// (see [`AppHost::new`]).
pub struct AppHost<'a> {
    cx: &'a mut AppContext,
}

impl<'a> AppHost<'a> {
    /// The host of `cx`, when its document is a library on disk open for writing (the AI connection never works
    /// on an in-memory session or a read-only library).
    pub fn new(cx: &'a mut AppContext) -> Option<Self> {
        (cx.doc.library_folder().is_some() && cx.doc.metadata_catalog().is_some() && !cx.doc.read_only())
            .then_some(Self { cx })
    }

    fn state(&self) -> Option<&HostState> {
        self.cx.state_ref::<HostState>()
    }
}

/// Runs the calls waiting for the window and the timers that are due (every frame).
pub fn pump(cx: &mut AppContext) {
    let gui = cx.state_ref::<super::AiState>().and_then(|s| s.connection.as_ref()).and_then(|c| c.gui());
    let now = Instant::now();
    let due: Vec<GuiTask> = {
        let state = cx.state::<HostState>();
        let (due, later): (Vec<_>, Vec<_>) =
            std::mem::take(&mut state.timers).into_iter().partition(|(at, _)| *at <= now);
        state.timers = later;
        due.into_iter().map(|(_, task)| task).collect()
    };
    let next = cx.state_ref::<HostState>().and_then(|s| s.timers.iter().map(|(at, _)| *at).min());
    if let Some(next) = next {
        cx.egui.request_repaint_after(next.saturating_duration_since(now));
    }
    let Some(mut host) = AppHost::new(cx) else {
        // No library open for writing: the calls get Busy from a closed queue.
        if let Some(gui) = gui {
            gui.close();
        }
        return;
    };
    for task in due {
        task(&mut host);
    }
    if let Some(gui) = gui {
        gui.pump(&mut host);
    }
}

/// `photo_service.english_error` of a failure (grainy-mcp's private `util::english`).
fn english(error: &grainy_service::Error) -> String {
    use grainy_service::{export::english_error, Error, Failure};
    match error {
        Error::Failed(failure) => english_error(failure),
        Error::Catalog(grainy_catalog::Error::Io(e)) => english_error(&Failure::io(e)),
        Error::Catalog(grainy_catalog::Error::Sql(_)) => {
            english_error(&Failure::new("OperationalError".to_owned(), error.to_string()))
        }
        other => english_error(&Failure::new(other.python_name().to_owned(), other.to_string())),
    }
}

fn running_any(cx: &AppContext, kinds: &[&str]) -> bool {
    kinds.iter().any(|k| cx.jobs.running(k))
}

impl Host for AppHost<'_> {
    fn closing(&self) -> bool {
        self.state().is_some_and(|s| s.closing)
    }
    fn modal_open(&self) -> bool {
        self.cx.modal_open() || eframe::egui::Popup::is_any_open(&self.cx.egui)
    }
    fn nested_loop(&self) -> bool {
        false
    }
    fn command_running(&self) -> bool {
        running_any(self.cx, &COMMAND_KINDS)
    }
    fn maintenance_running(&self) -> bool {
        self.cx.jobs.running("maintenance")
            || self.cx.state_ref::<crate::commands::files::FilesState>().is_some_and(|s| s.maintenance)
    }
    fn importing(&self) -> bool {
        running_any(self.cx, &["import", IMPORT_KIND]) || self.state().is_some_and(|s| !s.pending.is_empty())
    }
    fn import_scanning(&self) -> bool {
        self.cx.jobs.running("import")
    }
    fn export_running(&self) -> bool {
        self.cx.jobs.running("export")
    }
    fn interacting(&self) -> bool {
        self.cx.egui.egui_is_using_pointer() && self.cx.doc.edit().dirty()
    }
    fn unfinished_marks(&self) -> bool {
        let healing = self
            .cx
            .state_ref::<crate::tools::healing::HealingState>()
            .is_some_and(|s| s.pending.is_some() || !s.staged.is_empty());
        let brush = self.cx.state_ref::<crate::masks::MaskState>().is_some_and(|s| s.pending.is_some());
        healing || brush
    }
    fn tool_open(&self) -> bool {
        self.cx.tool() == Tool::Crop || self.cx.wb_picker
    }
    fn library_loading(&self) -> bool {
        self.cx.doc.listing_pending()
    }

    fn catalog(&self) -> &Catalog {
        // `AppHost::new` checked that the document has its catalogue; nothing replaces the document while a host
        // exists (it borrows the context).
        self.cx.doc.metadata_catalog().expect("a library document has its catalogue")
    }
    fn current_id(&self) -> Option<i64> {
        self.cx.doc.current().map(|p| p.id as i64)
    }
    fn unsaved(&self) -> bool {
        self.cx.doc.edit().dirty()
    }
    fn editor_settings(&self) -> Option<Value> {
        self.cx.doc.current().map(|_| Value::Object(self.cx.doc.edit().settings().0.clone()))
    }
    fn commit(&mut self) -> grainy_service::Result<()> {
        self.cx.doc.edit_mut().commit("보정 변경");
        self.cx.doc.flush().map_err(grainy_service::Error::Service)
    }
    fn save_keywords(&mut self) -> grainy_service::Result<()> {
        // Keywords are stored as they are typed (the metadata panel); nothing waits.
        Ok(())
    }
    fn selected_ids(&self) -> Vec<i64> {
        if self.cx.doc.listing_pending() {
            return Vec::new();
        }
        let photos = self.cx.doc.photos();
        self.cx.doc.selected_indices().iter().filter_map(|i| photos.get(*i)).map(|p| p.id as i64).collect()
    }
    fn presets(&self) -> grainy_service::Result<Vec<(String, Value)>> {
        let mut all = grainy_mcp::headless::built_in_presets();
        if let Value::Object(saved) = self.catalog().presets()? {
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
        u64::try_from(id).is_ok_and(|id| self.cx.doc.index_of(id).is_some())
    }

    fn refresh_command_edits(&mut self, ids: &[i64], label: &str) {
        let ids: Vec<u64> = ids.iter().filter_map(|i| u64::try_from(*i).ok()).collect();
        let had = self.cx.doc.edit().version();
        self.cx.doc.changed_elsewhere(&ids);
        if self.cx.doc.edit().version() != had {
            self.cx.set_before(false);
        }
        let count = super::ask::thousands(ids.len() as i64);
        self.cx.set_status(crate::text::tr_with("{0} · {1}장 변경", &[&tr(label), &count]));
    }
    fn refresh_lists(&mut self) {
        let ensure = self.cx.doc.current().is_none();
        self.cx.doc.request_refresh(ensure);
    }
    fn load_controls(&mut self) {
        if let Some(id) = self.cx.doc.current().map(|p| p.id) {
            self.cx.doc.changed_elsewhere(&[id]);
        }
    }
    fn activate(&mut self, id: i64) -> grainy_service::Result<()> {
        let index = u64::try_from(id).ok().and_then(|id| self.cx.doc.index_of(id));
        match index {
            Some(index) => {
                self.cx.select_photo(index);
                Ok(())
            }
            None if self.current_id() == Some(id) => Ok(()),
            None => Err(grainy_service::Error::Service(format!(
                "photo {id} is not in the current list view; show its folder in Grainy first"
            ))),
        }
    }
    fn set_mode(&mut self, edit: bool) {
        self.cx.set_mode(if edit { Mode::Develop } else { Mode::Library });
    }
    fn restore_selection(&mut self, ids: &[i64], current: i64) {
        let Some(index) = u64::try_from(current).ok().and_then(|c| self.cx.doc.index_of(c)) else { return };
        self.cx.doc.select(index);
        for id in ids.iter().filter(|i| **i != current) {
            if let Some(i) = u64::try_from(*id).ok().and_then(|id| self.cx.doc.index_of(id)) {
                self.cx.doc.toggle_selected(i);
            }
        }
    }

    fn spawn(&mut self, work: Work, done: Done) {
        let library = self.cx.doc.library_folder();
        self.cx.jobs.spawn(
            POOL_KIND,
            "",
            move |_| Ok(work()),
            move |cx: &mut AppContext, result: Result<Payload, String>| {
                // As `job_finished`: nothing reaches a window that closed (or now shows another library).
                if cx.state_ref::<HostState>().is_some_and(|s| s.closing) || cx.doc.library_folder() != library {
                    return;
                }
                let Ok(payload) = result else { return };
                if let Some(mut host) = AppHost::new(cx) {
                    done(&mut host, payload);
                }
            },
        );
    }
    fn later(&mut self, ms: u64, task: GuiTask) {
        let at = Instant::now() + Duration::from_millis(ms);
        self.cx.state::<HostState>().timers.push((at, task));
        self.cx.egui.request_repaint_after(Duration::from_millis(ms));
    }
    fn start_export(&mut self, export: ExportStart) -> grainy_service::Result<bool> {
        if self.cx.jobs.running("export") || export.ids.is_empty() {
            return Ok(false);
        }
        self.commit()?;
        let mut records = Vec::new();
        for id in &export.ids {
            if let Some(record) = self.catalog().photo(*id)? {
                records.push(record);
            }
        }
        let ExportStart { folder, options, cancel, progress, on_done, on_error, .. } = export;
        let library = self.cx.doc.library_folder();
        let total = records.len();
        self.cx.jobs.spawn(
            "export",
            tr("내보내기"),
            move |handle| {
                let workers = grainy_library::quick_export::export_concurrency_here(&records).unwrap_or(1).max(1);
                let report = |n: usize| {
                    progress(n);
                    handle.progress(n, total, crate::text::tr_with("내보내는 중… {0}/{1}", &[&n.to_string(), &total.to_string()]));
                };
                let result = grainy_service::export::export_records(
                    &records,
                    &folder,
                    &options,
                    Some(&cancel),
                    Some(&report),
                    &grainy_mcp::engine::EngineExporter,
                    true,
                    workers,
                );
                Ok(result.map_err(|e| english(&e)))
            },
            move |cx: &mut AppContext, result: Result<Result<Value, String>, String>| {
                if cx.doc.library_folder() != library {
                    return;
                }
                let Some(mut host) = AppHost::new(cx) else { return };
                match result.and_then(|r| r) {
                    Ok(mut result) => {
                        let outputs = result["outputs"].as_array().cloned().unwrap_or_default();
                        if !outputs.is_empty() {
                            let items: Vec<Value> = outputs
                                .iter()
                                .map(|o| json!({"path": o["path"], "photo_id": o["photo_id"], "edit_signature": o["edit_signature"]}))
                                .collect();
                            if grainy_catalog::workflow::record_exports(host.catalog(), &items).is_err() {
                                result["completion_error"] =
                                    "could not save export history; files already written stay".into();
                            }
                        }
                        on_done(&mut host, result);
                    }
                    Err(last) => on_error(&mut host, last),
                }
            },
        );
        Ok(true)
    }

    fn queue_import_files(&mut self, files: &[String], keys: &[String]) {
        let Some(folder) = self.cx.doc.library_folder() else { return };
        {
            let state = self.cx.state::<HostState>();
            state.quiet.extend(keys.iter().cloned());
            state.pending.extend(keys.iter().cloned());
        }
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        let keys = keys.to_vec();
        let library = self.cx.doc.library_folder();
        self.cx.jobs.spawn(
            IMPORT_KIND,
            tr("사진 가져오기"),
            move |handle| {
                let catalog = Catalog::new(&folder).map_err(|e| e.to_string())?;
                let progress = |done: usize, total: usize, _: &str| handle.progress(done, total, "");
                crate::document::catalog::import::import_files(&catalog, &files, &[], handle.cancel_flag(), &progress)
            },
            move |cx: &mut AppContext, result: Result<crate::document::catalog::import::Report, String>| {
                let same = cx.doc.library_folder() == library;
                let state = cx.state::<HostState>();
                for key in &keys {
                    state.pending.remove(key);
                }
                if !same {
                    return;
                }
                if let Err(message) = &result {
                    for key in &keys {
                        state.completion_errors.insert(key.clone(), message.clone());
                    }
                }
                let ensure = cx.doc.current().is_none();
                cx.doc.request_refresh(ensure);
                cx.state::<crate::session_state::ColumnState>().rescan();
            },
        );
    }
    fn import_waiting(&self, keys: &[String]) -> usize {
        self.state().map_or(0, |s| keys.iter().filter(|k| s.pending.contains(*k)).count())
    }
    fn release_import_quiet(&mut self, keys: &[String]) {
        let state = self.cx.state::<HostState>();
        for key in keys {
            if !state.pending.contains(key) {
                state.quiet.remove(key);
            }
        }
    }
    fn take_import_completion_errors(&mut self, keys: &[String]) -> Vec<String> {
        let state = self.cx.state::<HostState>();
        keys.iter().filter_map(|k| state.completion_errors.remove(k)).collect()
    }
    fn cancel_import(&mut self) {
        self.cx.jobs.cancel_kind(IMPORT_KIND);
        self.cx.jobs.cancel_kind("import");
    }
}
