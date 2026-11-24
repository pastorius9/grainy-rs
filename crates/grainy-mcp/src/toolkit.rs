//! Port of `luma/mcp_tools.py` class `Tools`: the 17 tool implementations on an [`McpService`], reaching the
//! window through a [`Gui`], plus `luma/mcp_io.py` (bounded file reads off the request thread).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use grainy_catalog::Catalog;
use grainy_service::auto::{reason, AutoKind};
use grainy_service::export::Longest;
use grainy_service::photo_edits::{EditEntry, PresetChoice};
use serde_json::{json, Map, Value};

use crate::host::{CallError, Gui, Host};
use crate::images::{self, Cell};
use crate::service::{capped, ready, safe_commit, write_batch_raw, Job, McpService, Ready};
use crate::tools::{call_checked, definitions, image_block, text_block, ToolError, ToolResult, Tools};
use crate::util::{casefold, english, get, id_of, is_video, path_text, repr, strip, truthy};

/// `PREVIEW_BYTES`.
pub const PREVIEW_BYTES: usize = 100_000;
/// `SHEET_BYTES`.
pub const SHEET_BYTES: usize = 200_000;
/// `SHEET_CELLS`.
pub const SHEET_CELLS: usize = 36;
/// `RESULT_BYTES`.
pub const RESULT_BYTES: usize = 300_000;
/// `RENDER_MARGIN`: seconds left below which no more previews are rendered.
pub const RENDER_MARGIN: f64 = 5.0;
/// `mcp_io.check_deadline`'s text.
pub const TIME_LIMIT: &str = "the time limit was reached; ask for fewer photos or try again";
/// The filter keys of list_photos (`FILTER`), in order.
pub const FILTER: [&str; 8] =
    ["folder", "recursive", "text", "rating_min", "rating_max", "flag", "label", "collection"];
const DESTINATION: [&str; 3] = ["destination", "path", "subfolder"];

type R<T> = Result<T, ToolError>;

impl From<grainy_service::Error> for ToolError {
    fn from(error: grainy_service::Error) -> Self {
        match error {
            grainy_service::Error::Service(text) => ToolError::Service(text),
            grainy_service::Error::Busy(text) => ToolError::Busy(text),
            other => ToolError::Other { last: english(&other) },
        }
    }
}

impl From<grainy_catalog::Error> for ToolError {
    fn from(error: grainy_catalog::Error) -> Self {
        grainy_service::Error::from(error).into()
    }
}

fn service_error(text: impl Into<String>) -> ToolError {
    ToolError::Service(text.into())
}

fn busy(text: impl Into<String>) -> ToolError {
    ToolError::Busy(text.into())
}

/// Port of `mcp_io.check_deadline`.
pub fn check_deadline(deadline: Instant) -> R<()> {
    if Instant::now() >= deadline {
        return Err(service_error(TIME_LIMIT));
    }
    Ok(())
}

fn left(deadline: Instant) -> f64 {
    deadline.saturating_duration_since(Instant::now()).as_secs_f64()
}

/// A cancel flag set after a delay (`threading.Timer(seconds, cancel.set)`); dropping it stops the timer.
struct Timer {
    flag: Arc<AtomicBool>,
    stop: Option<mpsc::Sender<()>>,
}

impl Timer {
    fn new(seconds: f64) -> Self {
        let flag = Arc::new(AtomicBool::new(false));
        let (stop, wait) = mpsc::channel::<()>();
        let set = flag.clone();
        std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = wait.recv_timeout(Duration::from_secs_f64(seconds.max(0.0))) {
                set.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        });
        Self { flag, stop: Some(stop) }
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn ids_of(value: &Value) -> Vec<i64> {
    value.as_array().map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default()
}

fn dedupe(ids: impl IntoIterator<Item = i64>) -> Vec<i64> {
    let mut out: Vec<i64> = Vec::new();
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn object(map: Map<String, Value>) -> Value {
    Value::Object(map)
}

/// `str(value)` of a JSON scalar as Python prints it in an f-string.
fn py_str(value: &Value) -> String {
    crate::pyjson::py_str(value)
}

/// Port of `Tools.one_target`.
fn one_target(arguments: &Value) -> R<&'static str> {
    let given: Vec<&'static str> =
        ["ids", "where", "scope"].into_iter().filter(|k| arguments.get(*k).is_some()).collect();
    if given.len() != 1 {
        return Err(service_error("give exactly one of ids, where or scope"));
    }
    Ok(given[0])
}

/// Port of `Tools.scope_ids` (GUI thread).
fn scope_ids(host: &dyn Host, scope: &str) -> R<Vec<i64>> {
    if scope == "current" {
        return host.current_id().map(|c| vec![c]).ok_or_else(|| service_error("no photo is open in Grainy"));
    }
    if host.library_loading() {
        return Err(busy("the photo list is still loading"));
    }
    Ok(dedupe(host.selected_ids()))
}

/// Port of `Tools.limit`.
fn limit(ids: Vec<i64>) -> R<Vec<i64>> {
    let max = grainy_service::MAX_TARGETS;
    if ids.len() > max {
        return Err(service_error(format!(
            "{} photos match; at most {max} per call. Narrow the filter or pass ids.",
            ids.len()
        )));
    }
    Ok(ids)
}

/// Port of `Tools.usable`.
fn usable(records: &[Value], errors: &[Value]) -> R<()> {
    if records.is_empty() {
        let shown: Vec<String> =
            errors.iter().take(5).map(|e| format!("{}: {}", py_str(get(e, "id")), py_str(get(e, "error")))).collect();
        let shown = if shown.is_empty() { "no photos matched".to_owned() } else { shown.join("; ") };
        return Err(service_error(format!("none of the photos can be used: {shown}")));
    }
    Ok(())
}

/// Port of `Tools.find_preset`: `(name, settings)` of one of Grainy's presets, by name in any case.
fn find_preset(host: &dyn Host, name: &str) -> R<(String, Value)> {
    let presets = host.presets()?;
    let wanted = casefold(strip(name));
    let found = presets.iter().find(|(k, _)| k == name).or_else(|| presets.iter().find(|(k, _)| casefold(k) == wanted));
    match found {
        Some((k, v)) => Ok((k.clone(), v.clone())),
        None => {
            let names: Vec<&str> = presets.iter().map(|(k, _)| k.as_str()).collect();
            let mut text =
                format!("no preset named {}; presets: {}", repr(name), names[..names.len().min(40)].join(", "));
            if names.len() > 40 {
                text += &format!(" and {} more", names.len() - 40);
            }
            Err(service_error(text))
        }
    }
}

/// Port of `Tools.find_snapshot`: the settings of the photo's newest snapshot called `name` (exactly, or else in
/// any case).
fn find_snapshot(catalog: &Catalog, ident: i64, name: &str) -> R<Option<Value>> {
    let rows = catalog.snapshots(ident)?;
    let text = |r: &Value| get(r, "name").as_str().unwrap_or("").to_owned();
    let wanted = casefold(strip(name));
    let row = rows.iter().find(|r| text(r) == name).or_else(|| rows.iter().find(|r| casefold(&text(r)) == wanted));
    match row {
        None => Ok(None),
        Some(r) => Ok(Some(grainy_service::normalized(get(r, "settings"))?)),
    }
}

/// `datetime.fromisoformat(created).astimezone().strftime('%Y-%m-%d %H:%M')`, `''` when it cannot be read.
fn local_minutes(created: &Value) -> String {
    let Some(text) = created.as_str() else { return String::new() };
    if let Ok(stamp) = chrono::DateTime::parse_from_rfc3339(text) {
        return grainy_core::local_time::wall_time(&stamp).format("%Y-%m-%d %H:%M").to_string();
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d"] {
        if let Ok(stamp) = chrono::NaiveDateTime::parse_from_str(text, format) {
            return stamp.format("%Y-%m-%d %H:%M").to_string();
        }
    }
    String::new()
}

/// The `{content, isError}` replies of the 17 tools, on an [`McpService`].
///
/// Port of `mcp_tools.Tools`. `gui` is the server's [`Gui`] (`None`: the connection is restarting and every call
/// that needs the window is Busy, as the oracle's tools without a GuiCall).
#[derive(Clone)]
pub struct GrainyTools {
    service: Arc<McpService>,
    gui: Option<Arc<dyn Gui>>,
}

impl GrainyTools {
    /// The tools of `service`, reaching the window through `gui`.
    pub fn new(service: Arc<McpService>, gui: Option<Arc<dyn Gui>>) -> Self {
        Self { service, gui }
    }

    /// The service.
    pub fn service(&self) -> &Arc<McpService> {
        &self.service
    }

    fn directory(&self) -> PathBuf {
        self.service.directory().to_path_buf()
    }

    /// Port of `Tools.gui`: `f` on the GUI thread through this request's own server.
    fn gui<T: Send + 'static>(
        &self,
        deadline: Instant,
        f: impl FnOnce(&mut dyn Host) -> R<T> + Send + 'static,
    ) -> R<T> {
        let Some(gui) = &self.gui else { return Err(busy("the AI connection is restarting")) };
        let (tx, rx) = mpsc::channel();
        let task = Box::new(move |host: &mut dyn Host| {
            let _ = tx.send(f(host));
        });
        gui.call(task, deadline).map_err(|e| match e {
            CallError::Busy(text) => ToolError::Busy(text),
            CallError::Unfinished => ToolError::Unfinished,
        })?;
        rx.try_recv().unwrap_or_else(|_| Err(ToolError::Other { last: "operation failed (internal error)".into() }))
    }

    /// Port of `Tools.alive`.
    fn alive(&self) -> R<()> {
        if !self.service.window_alive() {
            return Err(busy("Grainy is closing or reopening its window"));
        }
        Ok(())
    }

    /// Port of `mcp_io.run` (`Tools.background`): `f` on a worker thread, bounded by the deadline.
    fn background<T: Send + 'static>(&self, deadline: Instant, f: impl FnOnce() -> R<T> + Send + 'static) -> R<T> {
        check_deadline(deadline)?;
        let Some(permit) = self.service.io_slots.permit(0.0) else {
            return Err(busy("earlier file reads are still finishing"));
        };
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .spawn(move || {
                let _permit = permit;
                let result = check_deadline(deadline).and_then(|()| f());
                let _ = tx.send(result);
            })
            .map_err(|e| service_error(e.to_string()))?;
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(_) => Err(service_error(TIME_LIMIT)),
        }
    }

    /// Port of `Tools.query`: list_photos filters to ids on this thread, cancelled 5 s before the deadline.
    fn query(
        &self,
        filters: &Value,
        deadline: Instant,
        sort: Option<&str>,
        descending: bool,
        allow_empty: bool,
    ) -> R<Vec<i64>> {
        let timer = Timer::new(left(deadline) - RENDER_MARGIN);
        let ids = grainy_service::targets::query_ids(
            self.service.directory(),
            filters,
            sort,
            descending,
            Some(&timer.flag),
            allow_empty,
        )?;
        Ok(ids)
    }

    /// Port of `Tools.pre_ids`: ids or a where filter resolved on this thread; `None` for a scope.
    fn pre_ids(&self, arguments: &Value, deadline: Instant) -> R<Option<Vec<i64>>> {
        match one_target(arguments)? {
            "ids" => Ok(Some(ids_of(get(arguments, "ids")))),
            "where" => Ok(Some(self.query(get(arguments, "where"), deadline, None, false, false)?)),
            _ => Ok(None),
        }
    }

    /// Port of `Tools.reader_rows`.
    fn reader_rows(&self, ids: &[i64]) -> R<Vec<Option<Value>>> {
        let reader = Catalog::open_reader(self.service.directory())?;
        ids.iter().map(|i| reader.photo(*i).map_err(ToolError::from)).collect()
    }

    /// Port of `Tools.render_slot`.
    fn render_slot(&self, deadline: Instant) -> R<crate::service::Permit> {
        self.service.render_lock.permit(left(deadline).min(5.)).ok_or_else(|| busy("another preview is being made"))
    }

    /// Port of `Tools.job_reply`.
    fn job_reply(&self, job: Arc<Job>) -> R<Vec<Value>> {
        let state = job.state_name();
        if state == "done" {
            return Ok(vec![text_block(&object(job.result().unwrap_or_default()))]);
        }
        if state == "failed" || state == "cancelled" {
            let reason = job.error().filter(|e| !e.is_empty()).unwrap_or_else(|| "no reason given".into());
            return Err(ToolError::Job { message: format!("job {} {state}: {reason}", job.id), view: job.view() });
        }
        Ok(vec![text_block(&job.running_view())])
    }

    fn run(&self, name: &str, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        match name {
            "list_photos" => self.list_photos(a, deadline),
            "get_photo" => self.get_photo(a, deadline),
            "get_preview" => self.get_preview(a, deadline),
            "contact_sheet" => self.contact_sheet(a, deadline),
            "list_films" => self.list_films(a, deadline),
            "apply_edits" => self.apply_edits(a, deadline),
            "auto_tone" => self.start_auto(a, deadline, AutoKind::Tone, "AI 자동톤", "auto_tone", None),
            "auto_color" => {
                let strength = get(a, "strength").as_f64().map(|s| s / 100.0);
                self.start_auto(a, deadline, AutoKind::Color, "AI 자동 색", "auto_color", strength)
            }
            "invert_negatives" => {
                if truthy(get(a, "roll")) {
                    self.start_auto(a, deadline, AutoKind::Roll, "AI 네거티브 반전 (한 롤)", "invert_negatives", None)
                } else {
                    self.start_auto(a, deadline, AutoKind::Negative, "AI 네거티브 반전", "invert_negatives", None)
                }
            }
            "undo_last_batch" => self.undo_last_batch(a, deadline),
            "export_photos" => self.export_photos(a, deadline),
            "import_folder" => self.import_folder(a, deadline),
            "create_virtual_copies" => self.create_virtual_copies(a, deadline),
            "set_labels" => self.set_labels(a, deadline),
            "snapshot" => self.snapshot(a, deadline),
            "open_photo" => self.open_photo(a, deadline),
            "get_job" => self.get_job(a, deadline),
            other => Err(ToolError::Other { last: format!("operation failed (AttributeError: {other})") }),
        }
    }

    // --- read tools ---------------------------------------------------------------------------------------

    fn list_photos(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let mut filters = Map::new();
        for key in FILTER {
            if let Some(v) = a.get(key) {
                filters.insert(key.into(), v.clone());
            }
        }
        let scope = a.get("scope").and_then(Value::as_str).map(String::from);
        if scope.is_some() && !filters.is_empty() {
            return Err(service_error("give either filters or scope, not both"));
        }
        let offset = get(a, "offset").as_u64().unwrap_or(0) as usize;
        let limit = get(a, "limit").as_u64().unwrap_or(50) as usize;
        if get(a, "group") == "folders" {
            if scope.is_some() {
                return Err(service_error("group 'folders' takes no scope"));
            }
            let other: Vec<&str> = a
                .as_object()
                .map(|m| {
                    m.keys()
                        .map(String::as_str)
                        .filter(|k| !["group", "folder", "text", "offset", "limit"].contains(k))
                        .collect()
                })
                .unwrap_or_default();
            if !other.is_empty() {
                return Err(service_error(format!(
                    "group 'folders' takes only folder, text, offset and limit, not {}",
                    other.join(", ")
                )));
            }
            self.alive()?;
            let timer = Timer::new(left(deadline) - RENDER_MARGIN);
            let under = filters.get("folder").and_then(Value::as_str);
            let mut rows = grainy_service::targets::folder_rows(self.service.directory(), under, Some(&timer.flag))?;
            drop(timer);
            if let Some(text) = filters.get("text").and_then(Value::as_str) {
                let part = casefold(text).replace('\\', "/");
                rows.retain(|r| casefold(get(r, "path").as_str().unwrap_or("")).replace('\\', "/").contains(&part));
            }
            let mut out = Map::new();
            out.insert("total".into(), rows.len().into());
            out.insert("folders".into(), Value::Array(rows.iter().skip(offset).take(limit).cloned().collect()));
            if offset + limit < rows.len() {
                out.insert("next".into(), (offset + limit).into());
            }
            return Ok(vec![text_block(&object(out))]);
        }
        let ids = match scope {
            Some(scope) => {
                let service = self.service.clone();
                self.gui(deadline, move |host| {
                    ready(host, &service, &Ready::default())?;
                    scope_ids(host, &scope)
                })?
            }
            None => {
                self.alive()?;
                let sort = a.get("sort").and_then(Value::as_str);
                self.query(&object(filters), deadline, sort, truthy(get(a, "descending")), true)?
            }
        };
        let page: Vec<i64> = ids.iter().skip(offset).take(limit).copied().collect();
        let (dirs, rows) = grainy_service::targets::page_rows(self.service.directory(), &page)?;
        let mut out = Map::new();
        out.insert("total".into(), ids.len().into());
        out.insert("offset".into(), offset.into());
        if offset + limit < ids.len() {
            out.insert("next".into(), (offset + limit).into());
        }
        out.insert("dirs".into(), json!(dirs));
        out.insert("photos".into(), Value::Array(rows));
        Ok(vec![text_block(&object(out))])
    }

    /// The GUI half of the read tools (`Tools.read_commit`).
    fn read_commit(&self, ids: Vec<i64>, deadline: Instant) -> R<Option<String>> {
        let service = self.service.clone();
        self.gui(deadline, move |host| read_commit(host, &service, &ids))
    }

    fn get_photo(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let ids = ids_of(get(a, "ids"));
        let note = self.read_commit(ids.clone(), deadline)?;
        let rows = self.reader_rows(&ids)?;
        let present: Vec<Value> = rows.iter().flatten().cloned().collect();
        let (dirs, photos) = grainy_service::summary::summaries(&present)?;
        let mut out = Map::new();
        out.insert("dirs".into(), json!(dirs));
        out.insert("photos".into(), Value::Array(photos));
        let missing: Vec<i64> = ids.iter().zip(&rows).filter(|(_, r)| r.is_none()).map(|(i, _)| *i).collect();
        if !missing.is_empty() {
            out.insert("missing".into(), json!(missing));
        }
        if let Some(note) = note {
            out.insert("note".into(), note.into());
        }
        Ok(vec![text_block(&object(out))])
    }

    fn render(&self, row: &Value, size: usize, deadline: Instant) -> Result<ndarray::Array3<u8>, String> {
        let (record, directory) = (row.clone(), self.directory());
        let path = get(row, "path").as_str().unwrap_or("").to_owned();
        match self.background(deadline, move || Ok(crate::engine::render_preview(&record, size, &directory))) {
            Ok(Ok(pixels)) => Ok(pixels),
            Ok(Err(grainy_service::Error::Service(text))) | Err(ToolError::Service(text)) => Err(text),
            Ok(Err(error)) => Err(reason(&error, &path)),
            Err(ToolError::Busy(text)) => Err(reason(&grainy_service::Error::Busy(text), &path)),
            Err(_) => Err(reason(&grainy_service::Error::Runtime(String::new()), &path)),
        }
    }

    fn get_preview(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let ids = ids_of(get(a, "ids"));
        let size = get(a, "size").as_u64().unwrap_or(512) as usize;
        let quality = get(a, "quality").as_u64().unwrap_or(70) as u32;
        let note = self.read_commit(ids.clone(), deadline)?;
        let rows = self.reader_rows(&ids)?;
        let (mut images, mut previews, mut errors, mut total) = (Vec::new(), Vec::new(), Vec::new(), 0usize);
        let permit = self.render_slot(deadline)?;
        let outcome = (|| -> R<()> {
            for (ident, row) in ids.iter().zip(&rows) {
                let error = |text: &str| json!({"id": ident, "error": text});
                let Some(row) = row else {
                    errors.push(error("not in the catalog"));
                    continue;
                };
                if is_video(get(row, "path").as_str().unwrap_or("")) {
                    errors.push(error("video files are not supported"));
                    continue;
                }
                if left(deadline) < RENDER_MARGIN {
                    errors.push(error("not rendered in time; ask for fewer photos"));
                    continue;
                }
                let pixels = match self.render(row, size, deadline) {
                    Ok(p) => p,
                    Err(text) => {
                        errors.push(error(&text));
                        continue;
                    }
                };
                let (data, w, h) = encode_jpeg(&pixels, quality, Some(PREVIEW_BYTES))?;
                if total + data.len() > RESULT_BYTES {
                    errors.push(error("left out to keep the reply small; ask for fewer photos or a smaller size"));
                    continue;
                }
                total += data.len();
                images.push(image_block(&data));
                previews.push(json!({"id": ident, "w": w, "h": h}));
            }
            Ok(())
        })();
        drop(permit);
        outcome?;
        let mut out = Map::new();
        out.insert("previews".into(), Value::Array(previews));
        if !errors.is_empty() {
            out.insert("errors".into(), Value::Array(errors));
        }
        if let Some(note) = note {
            out.insert("note".into(), note.into());
        }
        images.push(text_block(&object(out)));
        Ok(images)
    }

    fn contact_sheet(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let kind = one_target(a)?;
        let cell = get(a, "cell").as_u64().unwrap_or(160) as usize;
        let saving = truthy(get(a, "save"));
        if !saving && (DESTINATION.iter().any(|k| a.get(*k).is_some()) || a.get("size").is_some()) {
            return Err(service_error("destination and size need save=true"));
        }
        let (ids, page, start) = if kind == "ids" {
            let ids = ids_of(get(a, "ids"));
            (ids.clone(), ids, 0)
        } else {
            let ids = if kind == "where" {
                self.query(get(a, "where"), deadline, None, false, false)?
            } else {
                let (service, scope) = (self.service.clone(), get(a, "scope").as_str().unwrap_or("").to_owned());
                self.gui(deadline, move |host| {
                    ready(host, &service, &Ready::default())?;
                    scope_ids(host, &scope)
                })?
            };
            let start = get(a, "offset").as_u64().unwrap_or(0) as usize;
            let count = get(a, "count").as_u64().unwrap_or(SHEET_CELLS as u64) as usize;
            let page: Vec<i64> = ids.iter().skip(start).take(count).copied().collect();
            (ids, page, start)
        };
        if page.is_empty() {
            return Err(service_error(if ids.is_empty() {
                "no photos to show"
            } else {
                "offset is past the last photo"
            }));
        }
        let variants = a.get("variants").and_then(Value::as_array).cloned().unwrap_or_default();
        if !variants.is_empty() && (ids.len() != 1 || page.len() != 1) {
            return Err(service_error("variants needs exactly one photo"));
        }
        let mut alternatives: Vec<Alternative> = Vec::new();
        for variant in &variants {
            if variant.get("film_look").is_some() == variant.get("preset").is_some() {
                return Err(service_error("each variant needs film_look or preset, not both"));
            }
            if let Some(look) = variant.get("film_look").and_then(Value::as_str) {
                let changes = grainy_service::edits::parse_changes(Some(&json!({"film_look": look})), None)?;
                let shown = if look.is_empty() { "No film".to_owned() } else { look.to_owned() };
                let label = variant.get("label").and_then(Value::as_str).map_or(shown.clone(), String::from);
                alternatives.push(Alternative { changes: Some(changes), preset: None, label, look: shown });
            } else {
                let wanted = get(variant, "preset").as_str().unwrap_or("").to_owned();
                let service = self.service.clone();
                let (name, base) = self.gui(deadline, move |host| {
                    if !service.window_alive() || host.closing() {
                        return Err(busy("Grainy is closing or reopening its window"));
                    }
                    find_preset(host, &wanted)
                })?;
                let label = variant.get("label").and_then(Value::as_str).map_or(name.clone(), String::from);
                alternatives.push(Alternative { changes: None, preset: Some(base), label, look: name });
            }
        }
        let note = self.read_commit(page.clone(), deadline)?;
        let folder = if saving { Some(self.destination(a, deadline, false)?) } else { None };
        let permit = self.render_slot(deadline)?;
        let (tools, total, arguments, paged) = (self.clone(), ids.len(), a.clone(), kind != "ids");
        let sheet_page = page.clone();
        let sheet = self.background(deadline, move || {
            let _permit = permit;
            tools.render_sheet(
                &sheet_page,
                &arguments,
                &alternatives,
                cell,
                saving,
                paged,
                start,
                total,
                note,
                deadline,
            )
        });
        let (data, mut out, saved) = sheet?;
        if let (Some(folder), Some((encoded, w, h))) = (folder, saved) {
            let own = self.service.own_folders();
            let saved = self.background(deadline, move || {
                let target = grainy_service::export::export_folder(&folder, None, &own, false)?;
                check_deadline(deadline)?;
                std::fs::create_dir_all(&target)
                    .map_err(|e| grainy_service::Error::Failed(grainy_service::Failure::io(&e)))?;
                let mut index = 1;
                loop {
                    check_deadline(deadline)?;
                    let path = target.join(grainy_service::export::safe_filename("contact-sheet", ".jpg", index));
                    match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                        Ok(mut file) => {
                            use std::io::Write;
                            file.write_all(&encoded)
                                .map_err(|e| grainy_service::Error::Failed(grainy_service::Failure::io(&e)))?;
                            return Ok(json!({"path": path_text(&path), "w": w, "h": h, "bytes": encoded.len()}));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => index += 1,
                        Err(e) => return Err(grainy_service::Error::Failed(grainy_service::Failure::io(&e)).into()),
                    }
                }
            })?;
            out.insert("saved".into(), saved);
        }
        Ok(vec![image_block(&data), text_block(&object(out))])
    }

    #[allow(clippy::too_many_arguments)]
    fn render_sheet(
        &self,
        page: &[i64],
        a: &Value,
        alternatives: &[Alternative],
        cell: usize,
        saving: bool,
        paged: bool,
        start: usize,
        total: usize,
        note: Option<String>,
        deadline: Instant,
    ) -> R<Sheet> {
        let directory = self.directory();
        let rows = self.reader_rows(page)?;
        let records: BTreeMap<i64, Value> = rows.into_iter().flatten().map(|r| (id_of(&r), r)).collect();
        let count = if alternatives.is_empty() { page.len() } else { alternatives.len() };
        let columns = images::MAX_COLUMNS.min(count);
        let side = columns.max(count.div_ceil(columns));
        let size = get(a, "size").as_u64().unwrap_or(2400) as usize;
        let saved_cell = (size - 2 * (side - 1)) / side;
        let requested = if saving { 2 * cell.max(saved_cell) } else { 2 * cell };
        let (mut cells, mut full, mut not_ready, mut errors) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let items: Vec<(i64, Option<&Alternative>)> = if alternatives.is_empty() {
            page.iter().map(|i| (*i, None)).collect()
        } else {
            alternatives.iter().map(|v| (page[0], Some(v))).collect()
        };
        for (ident, variant) in items {
            check_deadline(deadline)?;
            let mut record = records.get(&ident).cloned();
            let mut pixels = None;
            let mut look = String::new();
            if let Some(row) = record.as_mut().filter(|r| !is_video(get(r, "path").as_str().unwrap_or(""))) {
                if let Some(variant) = variant {
                    let settings = match (&variant.preset, &variant.changes) {
                        (Some(base), _) => grainy_service::edits::preset_settings(
                            base,
                            get(row, "settings"),
                            row,
                            &grainy_service::Plain,
                        )?,
                        (None, Some(changes)) => {
                            grainy_service::edits::edited(get(row, "settings"), changes, get(row, "info"), ident)?
                        }
                        (None, None) => get(row, "settings").clone(),
                    };
                    row["settings"] = settings;
                    look = variant.look.clone();
                } else {
                    let film = get(get(row, "settings"), "film_look").as_str().and_then(grainy_film::entry);
                    look = match film {
                        Some(entry) => entry.name.clone(),
                        None => {
                            let preset = get(get(row, "user_metadata"), grainy_service::PRESET_KEY);
                            if truthy(preset) {
                                py_str(preset)
                            } else {
                                String::new()
                            }
                        }
                    };
                }
                let attempt = (|| -> Result<Option<ndarray::Array3<u8>>, grainy_service::Error> {
                    let mut found = None;
                    if variant.is_none() && !saving {
                        found = crate::engine::cached_thumbnail(&directory, ident)?;
                    }
                    if found.is_none() && left(deadline) >= RENDER_MARGIN {
                        found = Some(crate::engine::render_preview(row, requested, &directory)?);
                    }
                    Ok(found)
                })();
                match attempt {
                    Ok(found) => pixels = found,
                    Err(error) => errors.push(json!({"id": ident,
                        "error": reason(&error, get(row, "path").as_str().unwrap_or(""))})),
                }
            }
            if pixels.is_none() {
                not_ready.push(ident);
            }
            let (rating, flag) = match &record {
                Some(r) => (get(r, "rating").as_f64().unwrap_or(0.0), get(r, "flag").as_i64().unwrap_or(0)),
                None => (0.0, 0),
            };
            let mark = images::mark(ident, rating, flag);
            let label = variant.map_or(mark.clone(), |v| v.label.clone());
            let name = record.as_ref().map_or(ident.to_string(), |r| py_str(get(r, "name")));
            let caption = if variant.is_some() && label != look { format!("{label} · {look}") } else { look.clone() };
            full.push(Cell { id: ident, pixels: pixels.clone(), label: format!("{mark}\n{name}\n{caption}") });
            cells.push(Cell { id: ident, pixels, label });
        }
        check_deadline(deadline)?;
        let sheet = images::contact_sheet(&cells, cell, false);
        let (data, _, _) = encode_jpeg(&sheet, 70, Some(SHEET_BYTES))?;
        let saved = if saving {
            let saved = images::contact_sheet(&full, saved_cell, true);
            let (h, w, _) = saved.dim();
            let (encoded, _, _) = encode_jpeg(&saved, 90, None)?;
            Some((encoded, w, h))
        } else {
            None
        };
        let mut out = Map::new();
        out.insert("ids".into(), json!(page));
        out.insert("columns".into(), columns.into());
        out.insert("total".into(), total.into());
        if !alternatives.is_empty() {
            out.insert("variants".into(), json!(alternatives.iter().map(|v| v.label.clone()).collect::<Vec<_>>()));
        }
        if paged && start + page.len() < total {
            out.insert("next".into(), (start + page.len()).into());
        }
        let missing: Vec<i64> = page.iter().filter(|i| !records.contains_key(i)).copied().collect();
        if !missing.is_empty() {
            out.insert("missing".into(), json!(missing));
        }
        if !not_ready.is_empty() {
            out.insert("not_ready".into(), json!(dedupe(not_ready.into_iter().filter(|i| records.contains_key(i)))));
        }
        if !errors.is_empty() {
            out.insert("errors".into(), Value::Array(errors));
        }
        if let Some(note) = note {
            out.insert("note".into(), note.into());
        }
        Ok((data, out, saved))
    }

    /// Port of `Tools.destination`: preferences on the GUI thread; resolving, checking and creating the folder on a
    /// bounded worker.
    fn destination(&self, a: &Value, deadline: Instant, create: bool) -> R<PathBuf> {
        let has_path = a.get("path").is_some();
        let destination =
            get(a, "destination").as_str().unwrap_or(if has_path { "path" } else { "default" }).to_owned();
        if (destination == "path") != has_path {
            return Err(service_error("destination 'path' needs path, and path needs destination 'path'"));
        }
        let service = self.service.clone();
        let last = destination == "last";
        let value = self.gui(deadline, move |host| {
            ready(host, &service, &Ready::default())?;
            if last {
                let previous = grainy_library::quick_export::previous(host.catalog(), false)
                    .map_err(|e| ToolError::Other { last: format!("operation failed ({})", library_class(&e)) })?;
                return Ok(previous.map_or(Value::Null, Value::Object));
            }
            Ok(service.preference("mcp_export_folder"))
        })?;
        let own = self.service.own_folders();
        let pictures = self.service.config().pictures.clone();
        let (path, subfolder) = (a.get("path").cloned(), a.get("subfolder").cloned());
        self.background(deadline, move || {
            let root = match destination.as_str() {
                "path" => grainy_service::export::named_folder(path.as_ref().unwrap_or(&Value::Null), &own, "path")?,
                "last" => {
                    if !truthy(&value) {
                        return Err(service_error(
                            "the person has not exported from Grainy yet; use destination 'default'",
                        ));
                    }
                    let root = PathBuf::from(get(&value, "folder").as_str().unwrap_or(""));
                    if !root.is_dir() {
                        return Err(service_error(format!(
                            "the folder of the person's last export ({}) is not available now, for example on a drive \
                             that is not connected; use destination 'default'",
                            path_text(&root)
                        )));
                    }
                    root
                }
                _ => match value.as_str().filter(|v| !v.is_empty() && Path::new(v).is_dir()) {
                    Some(saved) => PathBuf::from(saved),
                    None => pictures,
                },
            };
            let folder = grainy_service::export::export_folder(&root, subfolder.as_ref(), &own, false)?;
            check_deadline(deadline)?;
            if create {
                if let Err(error) = std::fs::create_dir_all(&folder) {
                    return Err(service_error(format!(
                        "the export folder cannot be created: {}",
                        grainy_service::export::english_error(&grainy_service::Failure::io(&error))
                    )));
                }
            }
            Ok(folder)
        })
    }

    fn list_films(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let kind = get(a, "type").as_str().unwrap_or("all").to_owned();
        if kind == "presets" {
            let service = self.service.clone();
            let (built_in, saved) = self.gui(deadline, move |host| {
                ready(host, &service, &Ready::default())?;
                let saved_names: Vec<String> =
                    host.catalog().presets()?.as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
                let all = host.presets()?;
                let built: Vec<String> =
                    all.iter().filter(|(n, _)| !saved_names.contains(n)).map(|(n, _)| n.clone()).collect();
                let saved: Vec<String> =
                    all.iter().filter(|(n, _)| saved_names.contains(n)).map(|(n, _)| n.clone()).collect();
                Ok((built, saved))
            })?;
            return Ok(vec![text_block(&json!({"built_in": built_in, "saved": saved}))]);
        }
        let mut films = Vec::new();
        for entry in grainy_film::catalog() {
            let raw = &entry.raw;
            let film_type = match raw.get("kind").and_then(Value::as_str) {
                Some("colour-negative") => Value::from("color"),
                Some("bw-negative") => Value::from("bw"),
                Some("colour-slide") => Value::from("slide"),
                _ => raw.get("kind").cloned().unwrap_or(Value::Null),
            };
            let cine = raw.get("group").and_then(Value::as_str) == Some("cine");
            if kind != "all" && film_type.as_str() != Some(kind.as_str()) && !(kind == "cine" && cine) {
                continue;
            }
            let mut film = Map::new();
            film.insert("name".into(), get(raw, "name").clone());
            film.insert("iso".into(), get(raw, "iso").clone());
            film.insert("type".into(), film_type);
            film.insert("maker".into(), get(raw, "maker").clone());
            if cine {
                film.insert("cine".into(), 1.into());
            }
            films.push(Value::Object(film));
        }
        Ok(vec![text_block(&json!({"films": films}))])
    }

    // --- write tools ----------------------------------------------------------------------------------------

    fn apply_edits(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let mut entries: Vec<Parsed> = Vec::new();
        for (index, entry) in get(a, "edits").as_array().cloned().unwrap_or_default().iter().enumerate() {
            let place = format!("arguments.edits[{index}]");
            let has = |k: &str| entry.get(k).is_some();
            if !["preset", "copy_from", "set", "add", "crop", "auto_straighten", "auto_crop"].iter().any(|k| has(k)) {
                return Err(service_error(format!(
                    "{place} needs preset, copy_from, set or add, crop or an automatic option"
                )));
            }
            if has("preset") && has("copy_from") {
                return Err(service_error(format!("{place}: give preset or copy_from, not both")));
            }
            if has("groups") && !has("copy_from") {
                return Err(service_error(format!("{place}: groups needs copy_from")));
            }
            let changes = if has("set") || has("add") {
                Some(grainy_service::edits::parse_changes(entry.get("set"), entry.get("add"))?)
            } else {
                None
            };
            let ids = self.pre_ids(entry, deadline)?;
            let ids = match ids {
                Some(ids) => Some(limit(ids)?),
                None => None,
            };
            let crop = match entry.get("crop") {
                Some(c) => Some(grainy_service::photo_edits::crop_value(c)?),
                None => None,
            };
            entries.push(Parsed {
                ids,
                scope: entry.get("scope").and_then(Value::as_str).map(String::from),
                preset: entry.get("preset").and_then(Value::as_str).map(String::from),
                copy_from: entry.get("copy_from").and_then(Value::as_i64),
                groups: entry
                    .get("groups")
                    .and_then(Value::as_array)
                    .map(|g| g.iter().filter_map(Value::as_str).map(String::from).collect()),
                changes,
                crop,
                auto_straighten: truthy(get(entry, "auto_straighten")),
                auto_crop: truthy(get(entry, "auto_crop")),
            });
        }
        let service = self.service.clone();
        let (entries, records, errors, sources) = self.gui(deadline, move |host| {
            ready(host, &service, &Ready::default())?;
            let mut entries = entries;
            let mut resolved = Vec::new();
            for entry in &entries {
                resolved.push(match &entry.ids {
                    Some(ids) => ids.clone(),
                    None => scope_ids(host, entry.scope.as_deref().unwrap_or(""))?,
                });
            }
            let targets = limit(dedupe(resolved.iter().flatten().copied()))?;
            let source_ids: BTreeSet<i64> = entries.iter().filter_map(|e| e.copy_from).collect();
            let mut affected: Vec<i64> = targets.clone();
            affected.extend(source_ids.iter().copied());
            let selection = entries.iter().any(|e| e.scope.as_deref() == Some("selection"));
            ready(host, &service, &Ready { write: true, ids: &affected, selection, ..Ready::default() })?;
            if host.current_id().is_some_and(|c| affected.contains(&c)) {
                if !safe_commit(host)? {
                    return Err(busy("the open photo cannot be saved yet"));
                }
                host.save_keywords()?;
            }
            let mut sources = BTreeMap::new();
            for ident in &source_ids {
                let Some(row) = host.catalog().photo(*ident)? else {
                    return Err(service_error(format!("copy_from photo {ident} is not in the catalog")));
                };
                if is_video(get(&row, "path").as_str().unwrap_or("")) {
                    return Err(service_error(format!("copy_from photo {ident} is a video")));
                }
                sources.insert(*ident, row);
            }
            let mut out = Vec::new();
            for (entry, ids) in entries.iter_mut().zip(resolved) {
                let preset = match &entry.preset {
                    Some(name) => {
                        let (name, base) = find_preset(host, name)?;
                        Some(PresetChoice { name, base })
                    }
                    None => None,
                };
                let keys = match entry.copy_from {
                    Some(_) => grainy_service::edits::copy_keys(entry.groups.as_deref())?,
                    None => BTreeSet::new(),
                };
                out.push(EditEntry {
                    ids,
                    copy_from: entry.copy_from,
                    keys,
                    preset,
                    changes: entry.changes.clone(),
                    auto_straighten: entry.auto_straighten,
                    auto_crop: entry.auto_crop,
                    crop: entry.crop.clone(),
                });
            }
            let (records, errors) = grainy_service::targets::freeze(host.catalog(), &targets)?;
            usable(&records, &errors)?;
            Ok((out, records, errors, sources))
        })?;
        let directory = self.directory();
        let (r2, s2) = (records.clone(), sources.clone());
        let (used, values, failures, presets) = self.background(deadline, move || {
            let mut deadline_check = || {
                check_deadline(deadline).map_err(|e| match e {
                    ToolError::Service(t) => grainy_service::Error::Service(t),
                    _ => grainy_service::Error::Service(TIME_LIMIT.into()),
                })
            };
            let mut automatic = |record: &Value, settings: &Value, straighten: bool, crop: bool| {
                crate::engine::automatic(record, settings, straighten, crop, &directory)
            };
            Ok(grainy_service::photo_edits::compose(
                &entries,
                &r2,
                &s2,
                &grainy_service::Plain,
                &mut deadline_check,
                &mut automatic,
            )?)
        })?;
        let service = self.service.clone();
        let reply = self.gui(deadline, move |host| {
            let mut ids: Vec<i64> = sources.keys().copied().collect();
            ids.extend(records.iter().map(id_of));
            ready(host, &service, &Ready { write: true, ids: &ids, ..Ready::default() })?;
            if host.current_id().is_some_and(|c| ids.contains(&c)) && !safe_commit(host)? {
                return Err(busy("the open photo cannot be saved yet"));
            }
            let mut check: Vec<Value> = Vec::new();
            for item in records.iter().chain(sources.values()) {
                let ident = id_of(item);
                match check.iter_mut().find(|r| id_of(r) == ident) {
                    Some(slot) => *slot = item.clone(),
                    None => check.push(item.clone()),
                }
            }
            if let Err(error) = grainy_service::batch::verify(host.catalog(), &check) {
                if error.conflict().is_some() {
                    return Err(service_error(
                        "a source or target changed during this call; nothing was applied; try again",
                    ));
                }
                return Err(error.into());
            }
            let mut all = errors;
            all.extend(failures);
            write_batch(host, &service, &used, &values, all, "AI 보정", "apply_edits", Some(&presets))
        })?;
        Ok(vec![text_block(&reply)])
    }

    fn undo_last_batch(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let redo = truthy(get(a, "redo"));
        let batch = a.get("batch").and_then(Value::as_str).map(String::from);
        let service = self.service.clone();
        let out = self.gui(deadline, move |host| {
            ready(host, &service, &Ready::default())?;
            let current: Vec<i64> = host.current_id().into_iter().collect();
            ready(host, &service, &Ready { write: true, ids: &current, ..Ready::default() })?;
            if host.current_id().is_some() {
                host.commit()?;
                host.save_keywords()?;
            }
            let (bid, tool, ids) = grainy_service::stack::undo_batch(host.catalog(), batch.as_deref(), redo)?;
            if !ids.is_empty() {
                if tool == "set_labels" {
                    host.refresh_lists();
                    if host.current_id().is_some_and(|c| ids.contains(&c)) {
                        host.load_controls();
                    }
                } else {
                    host.refresh_command_edits(&ids, if redo { "AI 다시 실행" } else { "AI 실행 취소" });
                }
            }
            let mut out = Map::new();
            out.insert(if redo { "redone" } else { "undone" }.into(), bid);
            out.insert("tool".into(), tool.into());
            out.insert("photos".into(), ids.len().into());
            Ok(object(out))
        })?;
        Ok(vec![text_block(&out)])
    }

    fn start_auto(
        &self,
        a: &Value,
        deadline: Instant,
        kind: AutoKind,
        label: &'static str,
        tool: &'static str,
        amount: Option<f64>,
    ) -> R<Vec<Value>> {
        let ids = match self.pre_ids(a, deadline)? {
            Some(ids) => Some(limit(ids)?),
            None => None,
        };
        let scope = a.get("scope").and_then(Value::as_str).map(String::from);
        let service = self.service.clone();
        let job = self.gui(deadline, move |host| {
            ready(host, &service, &Ready::default())?;
            let targets = limit(match ids {
                Some(ids) => ids,
                None => scope_ids(host, scope.as_deref().unwrap_or(""))?,
            })?;
            let selection = scope.as_deref() == Some("selection");
            ready(host, &service, &Ready { write: true, ids: &targets, selection, ..Ready::default() })?;
            if kind == AutoKind::Roll && targets.len() < 2 {
                return Err(service_error("roll=true needs at least 2 photos of the same roll"));
            }
            if host.current_id().is_some_and(|c| targets.contains(&c)) {
                host.commit()?;
                host.save_keywords()?;
            }
            let (records, errors) = grainy_service::targets::freeze(host.catalog(), &targets)?;
            usable(&records, &errors)?;
            if kind == AutoKind::Roll && records.len() < 2 {
                return Err(service_error("roll=true needs at least 2 photos that can be used"));
            }
            let mut value = amount;
            if kind == AutoKind::Color && value.is_none() {
                let saved = host.catalog().preference("auto_color_amount", json!(100))?;
                value = Some(match saved {
                    Value::Number(n) => n.as_f64().unwrap_or(100.0) / 100.0,
                    _ => 1.0,
                });
            }
            Ok(service.start_auto(host, kind, records, label, tool, value.unwrap_or(1.0), errors)?)
        })?;
        self.job_reply(job)
    }

    fn export_photos(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let naming = get(a, "naming").as_str().unwrap_or("{stem}_grainy").to_owned();
        grainy_service::export::check_naming(&Value::String(naming.clone()))?;
        let boxed = a.get("width").is_some() || a.get("height").is_some();
        if boxed && truthy(get(a, "longest")) {
            return Err(service_error("give longest, or width and/or height, not both"));
        }
        if a.get("fit").is_some() && !boxed {
            return Err(service_error("fit needs width and/or height"));
        }
        let size = if boxed {
            Longest::Box(
                get(a, "width").as_i64().unwrap_or(0),
                get(a, "height").as_i64().unwrap_or(0),
                get(a, "fit").as_str().unwrap_or("inside").to_owned(),
            )
        } else {
            Longest::Side(get(a, "longest").as_i64().unwrap_or(0))
        };
        let options = grainy_service::export::ExportOptions {
            format: get(a, "format").as_str().unwrap_or("JPEG").to_owned(),
            quality: get(a, "quality").as_i64().unwrap_or(90),
            longest: size.clone(),
            color_space: get(a, "color_space").as_str().unwrap_or("sRGB").to_owned(),
            keep_metadata: truthy(get(a, "keep_metadata")),
            naming: naming.clone(),
            grayscale: true,
            dither: true,
        };
        if options.format == "JPEG XL" && options.color_space != "sRGB" {
            return Err(service_error("JPEG XL is exported in sRGB only"));
        }
        let ids = match self.pre_ids(a, deadline)? {
            Some(ids) => Some(limit(ids)?),
            None => None,
        };
        let scope = a.get("scope").and_then(Value::as_str).map(String::from);
        let dry_run = truthy(get(a, "dry_run"));
        let service = self.service.clone();
        let (records, errors, note) = self.gui(deadline, move |host| {
            ready(host, &service, &Ready::default())?;
            let targets = limit(match ids {
                Some(ids) => ids,
                None => scope_ids(host, scope.as_deref().unwrap_or(""))?,
            })?;
            let selection = scope.as_deref() == Some("selection");
            ready(host, &service, &Ready { export: true, ids: &targets, selection, ..Ready::default() })?;
            let note = if dry_run { None } else { read_commit(host, &service, &targets)? };
            let mut records = Vec::new();
            for ident in &targets {
                records.push(host.catalog().photo(*ident)?);
            }
            if dry_run {
                let current = host.current_id();
                let editor = host.editor_settings();
                for record in records.iter_mut().flatten() {
                    if Some(id_of(record)) == current {
                        if let Some(settings) = &editor {
                            record["settings"] = settings.clone();
                        }
                    }
                }
            }
            let (valid, errors) = grainy_service::targets::freeze(host.catalog(), &targets)?;
            let allowed: Vec<i64> = valid.iter().map(id_of).collect();
            usable(&valid, &errors)?;
            let records: Vec<Value> = records.into_iter().flatten().filter(|r| allowed.contains(&id_of(r))).collect();
            Ok((records, errors, note))
        })?;
        let folder = self.destination(a, deadline, !dry_run)?;
        if dry_run {
            let plan = self.background(deadline, move || {
                let mut outputs = Vec::new();
                let mut failed = errors;
                let plans = grainy_service::export::export_plans(&records, &folder, &options.format, &naming)?;
                for (record, path) in plans {
                    let mut probe = |r: &Value| crate::engine::probe(r);
                    match grainy_service::export::planned_size(&record, &size, &mut probe) {
                        Ok((w, h)) => outputs
                            .push(json!({"photo_id": get(&record, "id"), "path": path, "size": [w, h],
                                                          "bytes": null})),
                        Err(error) => failed.push(json!({"id": get(&record, "id"),
                            "error": reason(&error, get(&record, "path").as_str().unwrap_or(""))})),
                    }
                }
                let mut out = Map::new();
                out.insert("dry_run".into(), true.into());
                out.insert("planned".into(), outputs.len().into());
                out.insert("folder".into(), path_text(&folder).into());
                if let Value::Object(files) = grainy_service::export::output_files(&outputs, 100) {
                    out.extend(files);
                }
                out.extend(capped(&failed));
                if let Some(note) = note {
                    out.insert("note".into(), note.into());
                }
                Ok(object(out))
            })?;
            return Ok(vec![text_block(&plan)]);
        }
        let service = self.service.clone();
        let job = self.gui(deadline, move |host| {
            ready(host, &service, &Ready { export: true, ..Ready::default() })?;
            let ids: Vec<i64> = records.iter().map(id_of).collect();
            Ok(service.start_export(host, ids, folder, options, errors)?)
        })?;
        self.job_reply(job)
    }

    fn import_folder(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let (path, own) = (get(a, "path").clone(), self.service.own_folders());
        let folder = self.background(deadline, move || {
            let folder = grainy_service::export::named_folder(&path, &own, "path")?;
            if !folder.is_dir() {
                return Err(service_error(format!(
                    "{} is not a folder on this computer (or not available now)",
                    path_text(&folder)
                )));
            }
            Ok(folder)
        })?;
        let recursive = truthy(get(a, "recursive"));
        let service = self.service.clone();
        let job = self.gui(deadline, move |host| {
            ready(host, &service, &Ready { write: true, ..Ready::default() })?;
            Ok(service.start_import(host, folder, recursive))
        })?;
        self.job_reply(job)
    }

    fn create_virtual_copies(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let ident = get(a, "id").as_i64().unwrap_or(0);
        let count = get(a, "count").as_u64().unwrap_or(1);
        let service = self.service.clone();
        let out = self.gui(deadline, move |host| {
            ready(host, &service, &Ready::default())?;
            ready(host, &service, &Ready { write: true, ids: &[ident], ..Ready::default() })?;
            let Some(photo) = host.catalog().photo(ident)? else {
                return Err(service_error(format!("photo {ident} is not in the catalog")));
            };
            if is_video(get(&photo, "path").as_str().unwrap_or("")) {
                return Err(service_error(format!("photo {ident} is a video")));
            }
            if host.current_id() == Some(ident) {
                host.commit()?;
                host.save_keywords()?;
            }
            let mut copies = Vec::new();
            for _ in 0..count {
                copies.push(host.catalog().virtual_copy(ident)?);
            }
            host.refresh_lists();
            Ok(json!({"copies": copies}))
        })?;
        Ok(vec![text_block(&out)])
    }

    fn set_labels(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let given = a.as_object().map_or(0, |m| m.len());
        if a.get("entries").is_some() && given != 1 {
            return Err(service_error("give entries or top-level targets and values, not both"));
        }
        let list = match a.get("entries").and_then(Value::as_array) {
            Some(entries) => entries.clone(),
            None => vec![a.clone()],
        };
        let mut entries: Vec<LabelTargets> = Vec::new();
        for entry in &list {
            let mut fields = Map::new();
            if let Some(rating) = entry.get("rating") {
                fields.insert("rating".into(), rating.clone());
            }
            if let Some(flag) = entry.get("flag").and_then(Value::as_str) {
                let value = grainy_service::FLAGS.iter().find(|(n, _)| *n == flag).map_or(0, |(_, v)| *v);
                fields.insert("flag".into(), value.into());
            }
            if let Some(label) = entry.get("label").and_then(Value::as_str) {
                let value = grainy_service::LABELS.iter().find(|(n, _)| *n == label).map_or("", |(_, v)| *v);
                fields.insert("label".into(), value.into());
            }
            if fields.is_empty() {
                return Err(service_error("give rating, flag and/or label"));
            }
            let ids = match self.pre_ids(entry, deadline)? {
                Some(ids) => Some(limit(ids)?),
                None => None,
            };
            entries.push((ids, entry.get("scope").and_then(Value::as_str).map(String::from), fields));
        }
        let service = self.service.clone();
        let out = self.gui(deadline, move |host| {
            ready(host, &service, &Ready::default())?;
            let mut resolved = Vec::new();
            for (ids, scope, fields) in &entries {
                let ids = match ids {
                    Some(ids) => ids.clone(),
                    None => scope_ids(host, scope.as_deref().unwrap_or(""))?,
                };
                resolved.push(grainy_service::labels::LabelEntry { ids, fields: fields.clone() });
            }
            let targets = limit(dedupe(resolved.iter().flat_map(|e| e.ids.iter().copied())))?;
            let selection = entries.iter().any(|(_, s, _)| s.as_deref() == Some("selection"));
            ready(host, &service, &Ready { write: true, ids: &targets, selection, ..Ready::default() })?;
            if host.current_id().is_some_and(|c| targets.contains(&c)) {
                host.commit()?;
                host.save_keywords()?;
            }
            let catalog = host.catalog();
            let mut tokens = || service.token(catalog);
            let (bid, changed, unchanged, errors) = grainy_service::labels::apply(catalog, &resolved, &mut tokens)?;
            if !changed.is_empty() {
                host.refresh_lists();
                if host.current_id().is_some_and(|c| changed.contains(&c)) {
                    host.load_controls();
                }
            }
            let mut out = Map::new();
            out.insert("batch".into(), bid.map_or(Value::Null, Value::String));
            out.insert("changed".into(), changed.len().into());
            out.insert("unchanged".into(), unchanged.into());
            out.extend(capped(&errors));
            Ok(object(out))
        })?;
        Ok(vec![text_block(&out)])
    }

    fn snapshot(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let action = get(a, "action").as_str().unwrap_or("").to_owned();
        let name = a.get("name").and_then(Value::as_str).map(String::from);
        if action != "list" && name.is_none() {
            return Err(service_error(format!("{action} needs name")));
        }
        if name.as_deref().is_some_and(|n| strip(n).is_empty()) {
            return Err(service_error("name must not be blank"));
        }
        let ids = match self.pre_ids(a, deadline)? {
            Some(ids) => Some(limit(ids)?),
            None => None,
        };
        let scope = a.get("scope").and_then(Value::as_str).map(String::from);
        if action == "list" {
            let ids = match ids {
                Some(ids) => ids,
                None => {
                    let service = self.service.clone();
                    self.gui(deadline, move |host| {
                        ready(host, &service, &Ready::default())?;
                        scope_ids(host, scope.as_deref().unwrap_or(""))
                    })?
                }
            };
            return Ok(vec![text_block(&self.snapshot_list(&ids, name.as_deref())?)]);
        }
        let service = self.service.clone();
        let out = self.gui(deadline, move |host| {
            ready(host, &service, &Ready::default())?;
            let targets = limit(match ids {
                Some(ids) => ids,
                None => scope_ids(host, scope.as_deref().unwrap_or(""))?,
            })?;
            let selection = scope.as_deref() == Some("selection");
            ready(host, &service, &Ready { write: true, ids: &targets, selection, ..Ready::default() })?;
            if host.current_id().is_some_and(|c| targets.contains(&c)) {
                host.commit()?;
                host.save_keywords()?;
            }
            let (records, mut errors) = grainy_service::targets::freeze(host.catalog(), &targets)?;
            let name = name.unwrap_or_default();
            if action == "save" {
                usable(&records, &errors)?;
                for record in &records {
                    host.catalog().snapshot(id_of(record), strip(&name))?;
                }
                let mut out = Map::new();
                out.insert("saved".into(), records.len().into());
                out.extend(capped(&errors));
                return Ok(object(out));
            }
            let (mut used, mut values) = (Vec::new(), Vec::new());
            for record in records {
                match find_snapshot(host.catalog(), id_of(&record), &name)? {
                    None => errors
                        .push(json!({"id": id_of(&record), "error": format!("no snapshot named {}", repr(&name))})),
                    Some(stored) => {
                        used.push(record);
                        values.push(stored);
                    }
                }
            }
            write_batch(host, &service, &used, &values, errors, "AI 스냅샷 복원", "snapshot", None)
        })?;
        Ok(vec![text_block(&out)])
    }

    /// Port of `Tools.snapshot_list`: `{photos: [{id, snapshots: [{name, created}]}]}`, newest first, at most 20 a
    /// photo; read on this thread.
    fn snapshot_list(&self, ids: &[i64], name: Option<&str>) -> R<Value> {
        let reader = Catalog::open_reader(self.service.directory())?;
        let wanted = name.map(|n| casefold(strip(n)));
        let mut found: BTreeMap<i64, Vec<Value>> = BTreeMap::new();
        let mut present: BTreeSet<i64> = BTreeSet::new();
        for part in ids.chunks(500) {
            let marks = vec!["?"; part.len()].join(",");
            let rows = reader.rows(
                &format!("SELECT photo_id,name,created FROM snapshots WHERE photo_id IN ({marks}) ORDER BY id DESC"),
                grainy_catalog::rusqlite::params_from_iter(part),
            )?;
            for row in rows {
                let label = get(&row, "name").as_str().unwrap_or("");
                if wanted.as_ref().is_some_and(|w| casefold(label) != *w) {
                    continue;
                }
                found
                    .entry(id_of(&json!({"id": get(&row, "photo_id")})))
                    .or_default()
                    .push(json!({"name": get(&row, "name"), "created": local_minutes(get(&row, "created"))}));
            }
            let rows = reader.rows(
                &format!("SELECT id FROM photos WHERE id IN ({marks})"),
                grainy_catalog::rusqlite::params_from_iter(part),
            )?;
            present.extend(rows.iter().map(id_of));
        }
        let photos: Vec<Value> = ids
            .iter()
            .filter(|i| present.contains(i))
            .map(|i| json!({"id": i, "snapshots": found.get(i).map(|l| l.iter().take(20).cloned().collect::<Vec<_>>()).unwrap_or_default()}))
            .collect();
        let mut out = Map::new();
        out.insert("photos".into(), Value::Array(photos));
        let missing: Vec<i64> = ids.iter().filter(|i| !present.contains(i)).copied().collect();
        if !missing.is_empty() {
            out.insert("missing".into(), json!(missing));
        }
        Ok(object(out))
    }

    fn open_photo(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let ident = get(a, "id").as_i64().unwrap_or(0);
        let edit = get(a, "view").as_str().unwrap_or("edit") == "edit";
        let service = self.service.clone();
        let out = self.gui(deadline, move |host| {
            ready(host, &service, &Ready::default())?;
            let current: Vec<i64> = host.current_id().into_iter().collect();
            ready(host, &service, &Ready { write: true, ids: &current, ..Ready::default() })?;
            let Some(photo) = host.catalog().photo(ident)? else {
                return Err(service_error(format!("photo {ident} is not in the catalog")));
            };
            if is_video(get(&photo, "path").as_str().unwrap_or("")) {
                return Err(service_error(format!("photo {ident} is a video; Grainy cannot open it")));
            }
            host.activate(ident)?;
            host.set_mode(edit);
            let mut out = Map::new();
            out.insert("opened".into(), ident.into());
            if host.listed(ident) {
                host.restore_selection(&[ident], ident);
            } else {
                out.insert("note".into(), "not in the current list view (filters or another folder are shown)".into());
            }
            Ok(object(out))
        })?;
        Ok(vec![text_block(&out)])
    }

    fn get_job(&self, a: &Value, deadline: Instant) -> R<Vec<Value>> {
        let ident = get(a, "job").as_str().unwrap_or("");
        let Some(job) = self.service.job(ident) else {
            return Err(service_error("unknown job (jobs are kept for this Grainy session only)"));
        };
        let wait = get(a, "wait").as_f64().unwrap_or(20.0);
        job.wait(wait.min(left(deadline) - 2.0));
        Ok(vec![text_block(&job.view())])
    }
}

/// The class name of a library error for `english_error`.
fn library_class(error: &grainy_library::Error) -> String {
    let text = format!("{error:?}");
    text.split(['(', ' ', '{']).next().unwrap_or("Error").to_owned()
}

/// The reply image, the reply text and the saved sheet (bytes, width, height) of a contact sheet.
type Sheet = (Vec<u8>, Map<String, Value>, Option<(Vec<u8>, usize, usize)>);
/// One set_labels entry: ids (or `None` for a scope), the scope and the values.
type LabelTargets = (Option<Vec<i64>>, Option<String>, Map<String, Value>);

/// One contact-sheet variant: a film look's changes or a preset's settings, its label and look name.
#[derive(Clone)]
struct Alternative {
    changes: Option<grainy_service::Changes>,
    preset: Option<Value>,
    label: String,
    look: String,
}

/// One apply_edits entry after the checks on the request thread.
struct Parsed {
    ids: Option<Vec<i64>>,
    scope: Option<String>,
    preset: Option<String>,
    copy_from: Option<i64>,
    groups: Option<Vec<String>>,
    changes: Option<grainy_service::Changes>,
    crop: Option<Value>,
    auto_straighten: bool,
    auto_crop: bool,
}

/// `photo_images.jpeg` with the dimensions of the image that was encoded last (what the reply's `w`/`h` read
/// back from the JPEG); the pixels first handed to it go to the service's observer (tests).
fn encode_jpeg(image: &ndarray::Array3<u8>, quality: u32, max_bytes: Option<usize>) -> R<(Vec<u8>, usize, usize)> {
    crate::observe::seen(image);
    images::jpeg_with(image, quality, max_bytes, &mut |_| {})
        .map_err(|e| ToolError::Other { last: format!("operation failed (OSError: {e})") })
}

/// The GUI half of the read tools (`Tools.read_commit`): save the open photo's pending change when it is targeted
/// and safe; otherwise the note the reply carries.
fn read_commit(host: &mut dyn Host, service: &McpService, ids: &[i64]) -> R<Option<String>> {
    ready(host, service, &Ready::default())?;
    if host.current_id().is_some_and(|c| ids.contains(&c)) && !safe_commit(host)? {
        return Ok(Some("the open photo has unsaved changes; values shown are the last saved ones".into()));
    }
    Ok(None)
}

/// Port of `Tools.write_batch` (GUI thread): one undoable batch and the reply apply_edits and snapshot restore give.
#[allow(clippy::too_many_arguments)]
fn write_batch(
    host: &mut dyn Host,
    service: &McpService,
    used: &[Value],
    values: &[Value],
    errors: Vec<Value>,
    label: &str,
    tool: &str,
    presets: Option<&BTreeMap<i64, String>>,
) -> R<Value> {
    usable(used, &errors)?;
    let (bid, changed) = write_batch_raw(host, service, used, values, label, tool, presets)?;
    if !changed.is_empty() {
        host.refresh_command_edits(&changed, label);
    }
    let mut out = Map::new();
    out.insert("batch".into(), bid.map_or(Value::Null, Value::String));
    out.insert("changed".into(), changed.len().into());
    out.insert("unchanged".into(), (used.len() - changed.len()).into());
    out.extend(capped(&errors));
    Ok(object(out))
}

impl Tools for GrainyTools {
    fn definitions(&self) -> &[Value] {
        definitions()
    }

    /// Port of `Tools.call`: check the arguments, run the tool and map its outcome to `{content, isError}`.
    fn call(&self, name: &str, arguments: &Value, _modern: bool, deadline: Instant) -> ToolResult {
        call_checked(name, arguments, || {
            let outcome = self.run(name, arguments, deadline);
            if let Err(ToolError::Other { last }) = &outcome {
                self.service.note(name, &format!("failed: {last}"));
            }
            outcome
        })
    }
}
