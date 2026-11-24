//! Port of `luma/mcp_service.py` without Qt: readiness (`ready`, `safe_commit`), jobs (`Job`, the registry,
//! `capped`, `start_auto`, `start_export`, `start_import`), the service's limits, folders and activity log.
//! The server's lifetime and the bridge files are in [`crate::bridge`]; the window parts in [`crate::host`].

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use grainy_catalog::Catalog;
use serde_json::{json, Map, Value};

use crate::engine;
use crate::host::{ExportStart, Host, Payload};
use crate::util::{eq, get, lock};

/// `GRACE_SECONDS`: GUI work that already started may finish this long after the request deadline.
pub const GRACE_SECONDS: f64 = 8.0;
/// `QUEUE_SECONDS`: the longest wait for the GUI thread to start a call.
pub const QUEUE_SECONDS: f64 = 10.0;
/// `RETRY_SECONDS`: a finished job waits this long for Grainy to be ready to write.
pub const RETRY_SECONDS: f64 = 60.0;
/// `MAX_IMPORT`: image files one import_folder call takes.
pub const MAX_IMPORT: usize = 10000;
/// `MAX_REPORTED`: photo ids listed in one job result.
pub const MAX_REPORTED: usize = 500;
/// `DEFAULT_PORT`.
pub const DEFAULT_PORT: u16 = 47821;
/// `STOP_SECONDS`: stop() waits this long for requests in flight.
pub const STOP_SECONDS: f64 = 3.0;
/// `CLOSED`: what a call gets from a stopped server's GuiCall.
pub const CLOSED: &str = "Grainy is closing or its AI connection was restarted";
/// Jobs kept for get_job.
pub const KEPT_JOBS: usize = 20;

/// A Python `Busy` or `ServiceError` raised by the service's own steps.
pub type Fault = grainy_service::Error;

fn busy(text: impl Into<String>) -> Fault {
    Fault::Busy(text.into())
}

/// A counting semaphore (`threading.BoundedSemaphore`).
pub struct Semaphore {
    free: Mutex<usize>,
    signal: Condvar,
}

pub(crate) struct Permit(Arc<Semaphore>);
impl Drop for Permit {
    fn drop(&mut self) {
        self.0.release();
    }
}

impl Semaphore {
    pub(crate) fn permit(self: &Arc<Self>, seconds: f64) -> Option<Permit> {
        self.acquire(seconds).then(|| Permit(self.clone()))
    }
    /// A semaphore with `count` permits.
    pub fn new(count: usize) -> Self {
        Self { free: Mutex::new(count), signal: Condvar::new() }
    }
    /// `acquire(timeout=seconds)`; `0` does not wait.
    pub fn acquire(&self, seconds: f64) -> bool {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
        let mut free = lock(&self.free);
        loop {
            if *free > 0 {
                *free -= 1;
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            free = self.signal.wait_timeout(free, deadline - now).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }
    /// `release()`.
    pub fn release(&self) {
        *lock(&self.free) += 1;
        self.signal.notify_one();
    }
}

/// Where the service's files are and what it may not write into (`McpService.directory`, `own_folders`,
/// `pictures_folder`).
#[derive(Clone, Debug)]
pub struct Config {
    /// The library folder (`session.data_dir`).
    pub directory: PathBuf,
    /// The settings folder (`Path(preferences.path).parent`): the bridge files live in its `mcp` folder.
    pub settings_folder: PathBuf,
    /// The program's own folders (the oracle's source folder and, frozen, the executable's).
    pub program_folders: Vec<PathBuf>,
    /// `pictures_folder()`: Pictures/Grainy, the default folder for AI exports.
    pub pictures: PathBuf,
}

impl Config {
    /// A configuration with the program folder of this executable and Pictures/Grainy of this user.
    pub fn new(directory: PathBuf, settings_folder: PathBuf) -> Self {
        let program = std::env::current_exe().ok().and_then(|p| p.parent().map(PathBuf::from)).into_iter().collect();
        Self { directory, settings_folder, program_folders: program, pictures: default_pictures() }
    }
}

/// Port of `pictures_folder`: the user's Pictures folder (or `~/Pictures`) plus `Grainy`.
pub fn default_pictures() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from);
    home.unwrap_or_default().join("Pictures").join("Grainy")
}

/// Timings the tests shorten (the oracle's module constants, patched by its tests).
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// `RETRY_SECONDS`.
    pub retry_seconds: f64,
}

impl Default for Limits {
    fn default() -> Self {
        Self { retry_seconds: RETRY_SECONDS }
    }
}

/// One job's mutable state.
#[derive(Clone, Debug)]
struct JobState {
    total: i64,
    done: i64,
    state: String,
    phase: Option<String>,
    result: Option<Map<String, Value>>,
    error: Option<String>,
}

/// Port of `mcp_service.Job`: a long-running tool call, polled with get_job.
pub struct Job {
    /// `j1`, `j2`, ...
    pub id: String,
    /// The tool that started it.
    pub tool: String,
    generation: u64,
    inner: Mutex<JobState>,
    finished: Condvar,
    /// Set by the stop button, the connection going off, or the window closing.
    pub cancel: Arc<AtomicBool>,
    stopped: AtomicBool,
}

impl Job {
    fn new(id: String, tool: &str, total: i64, generation: u64) -> Self {
        let state = JobState { total, done: 0, state: "running".into(), phase: None, result: None, error: None };
        Self {
            id,
            tool: tool.into(),
            generation,
            inner: Mutex::new(state),
            finished: Condvar::new(),
            cancel: Arc::new(AtomicBool::new(false)),
            stopped: AtomicBool::new(false),
        }
    }

    fn state(&self) -> MutexGuard<'_, JobState> {
        lock(&self.inner)
    }

    /// `job.state`: running, done, failed or cancelled.
    pub fn state_name(&self) -> String {
        self.state().state.clone()
    }

    /// Whether the job is still running.
    pub fn running(&self) -> bool {
        self.state().state == "running"
    }

    /// `job.done = n` (progress, any thread).
    pub fn set_done(&self, done: i64) {
        self.state().done = done;
    }

    /// `job.total = n`.
    pub fn set_total(&self, total: i64) {
        self.state().total = total;
    }

    /// `job.total`.
    pub fn total(&self) -> i64 {
        self.state().total
    }

    /// `job.phase = text`.
    pub fn set_phase(&self, phase: Option<&str>) {
        self.state().phase = phase.map(String::from);
    }

    /// `job.result`.
    pub fn result(&self) -> Option<Map<String, Value>> {
        self.state().result.clone()
    }

    /// `job.error`.
    pub fn error(&self) -> Option<String> {
        self.state().error.clone()
    }

    /// The stop button (`McpWindowState.stop_job`): `job.stopped = True; job.cancel.set()`.
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::SeqCst);
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// `job.stopped`.
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::SeqCst)
    }

    /// `job.finished.wait(seconds)`: true when the job is no longer running.
    pub fn wait(&self, seconds: f64) -> bool {
        let deadline = Instant::now() + Duration::from_secs_f64(seconds.max(0.0));
        let mut state = self.state();
        while state.state == "running" {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state =
                self.finished.wait_timeout(state, deadline - now).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
        true
    }

    /// Port of `Job.view`: what get_job returns.
    pub fn view(&self) -> Value {
        let s = self.state();
        let mut out = Map::new();
        out.insert("job".into(), self.id.clone().into());
        out.insert("tool".into(), self.tool.clone().into());
        out.insert("state".into(), s.state.clone().into());
        out.insert("done".into(), s.done.into());
        out.insert("total".into(), s.total.into());
        if let Some(phase) = s.phase.as_ref().filter(|p| !p.is_empty() && s.state == "running") {
            out.insert("phase".into(), phase.clone().into());
        }
        if let Some(result) = &s.result {
            for (k, v) in result {
                out.insert(k.clone(), v.clone());
            }
        }
        if let Some(error) = s.error.as_ref().filter(|e| !e.is_empty()) {
            out.insert("error".into(), error.clone().into());
        }
        Value::Object(out)
    }

    /// The reply of a tool that started this job and waited for it (`Tools.job_reply` while running).
    pub fn running_view(&self) -> Value {
        let s = self.state();
        let mut out = Map::new();
        out.insert("job".into(), self.id.clone().into());
        out.insert("state".into(), s.state.clone().into());
        out.insert("done".into(), s.done.into());
        out.insert("total".into(), s.total.into());
        if let Some(phase) = s.phase.as_ref().filter(|p| !p.is_empty()) {
            out.insert("phase".into(), phase.clone().into());
        }
        Value::Object(out)
    }
}

/// Port of `capped`: per-photo error lists of at most 20 entries, plus `more_errors`.
pub fn capped(errors: &[Value]) -> Map<String, Value> {
    let mut out = Map::new();
    out.insert("errors".into(), Value::Array(errors.iter().take(20).cloned().collect()));
    if errors.len() > 20 {
        out.insert("more_errors".into(), (errors.len() - 20).into());
    }
    out
}

/// Where the service's batch ids come from (`secrets.token_hex(3)`).
pub type TokenSource = Box<dyn FnMut(&Catalog) -> grainy_service::Result<String> + Send>;

struct State {
    running: bool,
    generation: u64,
    job: Option<Arc<Job>>,
    jobs: VecDeque<Arc<Job>>,
    job_count: u64,
    snapshotted: HashSet<i64>,
    activity: VecDeque<(String, String, String)>,
    window_alive: bool,
}

/// Port of `McpService` without the Qt object: what the tools share across requests and servers.
pub struct McpService {
    config: Config,
    limits: Mutex<Limits>,
    preferences: Mutex<Map<String, Value>>,
    state: Mutex<State>,
    tokens: Mutex<TokenSource>,
    local_time: Box<dyn Fn() -> String + Send + Sync>,
    /// `render_lock`: one preview or contact sheet at a time.
    pub render_lock: Arc<Semaphore>,
    /// `io_slots`: bounded file reads (`mcp_io.run`).
    pub io_slots: Arc<Semaphore>,
    /// `calls`: tool calls handled at once (`mcp_http.SlotTools`).
    pub calls: Arc<Semaphore>,
}

impl McpService {
    /// A running service (`start()` done) for the library of `config`; batch ids from SQLite's random bytes and the
    /// local clock for snapshot names.
    pub fn new(config: Config) -> Arc<Self> {
        Self::with_sources(
            config,
            Box::new(grainy_service::stack::random_token),
            Box::new(|| chrono::Local::now().format("%Y-%m-%d %H:%M").to_string()),
        )
    }

    /// A service with injected batch ids and local time (`time.strftime('%Y-%m-%d %H:%M')`).
    pub fn with_sources(
        config: Config,
        tokens: TokenSource,
        local_time: Box<dyn Fn() -> String + Send + Sync>,
    ) -> Arc<Self> {
        Arc::new(Self {
            config,
            limits: Mutex::new(Limits::default()),
            preferences: Mutex::new(Map::new()),
            state: Mutex::new(State {
                running: true,
                generation: 1,
                job: None,
                jobs: VecDeque::new(),
                job_count: 0,
                snapshotted: HashSet::new(),
                activity: VecDeque::new(),
                window_alive: true,
            }),
            tokens: Mutex::new(tokens),
            local_time,
            render_lock: Arc::new(Semaphore::new(1)),
            io_slots: Arc::new(Semaphore::new(4)),
            calls: Arc::new(Semaphore::new(4)),
        })
    }

    /// The configuration.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// `service.directory`: the library folder.
    pub fn directory(&self) -> &std::path::Path {
        &self.config.directory
    }

    /// Port of `own_folders`: the library, the settings folder and the program; AI exports and imports never go
    /// there.
    pub fn own_folders(&self) -> Vec<PathBuf> {
        let mut out = vec![self.config.directory.clone(), self.config.settings_folder.clone()];
        out.extend(self.config.program_folders.iter().cloned());
        out
    }

    /// The limits in force.
    pub fn limits(&self) -> Limits {
        *lock(&self.limits)
    }

    /// Change the limits (tests).
    pub fn set_limits(&self, limits: Limits) {
        *lock(&self.limits) = limits;
    }

    /// `preferences.values.get(key)` (the settings file's values the tools read: `mcp_export_folder`).
    pub fn preference(&self, key: &str) -> Value {
        lock(&self.preferences).get(key).cloned().unwrap_or(Value::Null)
    }

    /// Set one preference value as the settings file has it.
    pub fn set_preference(&self, key: &str, value: Value) {
        lock(&self.preferences).insert(key.into(), value);
    }

    /// The window exists and is not closing (`session.window` and `not w.closing`, read from a request thread by
    /// `Tools.alive`). The application keeps this up to date when its window closes or is rebuilt.
    pub fn window_alive(&self) -> bool {
        lock(&self.state).window_alive
    }

    /// See [`McpService::window_alive`].
    pub fn set_window_alive(&self, alive: bool) {
        lock(&self.state).window_alive = alive;
    }

    /// `service.running`.
    pub fn running(&self) -> bool {
        lock(&self.state).running
    }

    /// `service.generation`.
    pub fn generation(&self) -> u64 {
        lock(&self.state).generation
    }

    /// The connection starts again (`start()` after `stop()`, or a new port): a new generation; earlier jobs'
    /// results are no longer written.
    pub fn restart(&self) {
        lock(&self.state).generation += 1;
    }

    /// Port of `stop()` for the service's own state: a running job is cancelled; `running` goes off.
    pub fn stop(&self) {
        let mut state = lock(&self.state);
        state.running = false;
        state.generation += 1;
        if let Some(job) = state.job.as_ref().filter(|j| j.running()) {
            job.cancel.store(true, Ordering::SeqCst);
        }
        drop(state);
        self.note("server", "stopped");
    }

    /// Port of `start()` for the service's own state.
    pub fn start(&self) {
        let mut state = lock(&self.state);
        if !state.running {
            state.running = true;
            state.generation += 1;
        }
        drop(state);
        self.note("server", "started");
    }

    /// Port of `note`: one line of the recent activity (time, name, outcome).
    pub fn note(&self, name: &str, outcome: &str) {
        let line = ((self.local_time)(), name.chars().take(64).collect(), outcome.chars().take(200).collect());
        let mut state = lock(&self.state);
        state.activity.push_back(line);
        while state.activity.len() > 50 {
            state.activity.pop_front();
        }
    }

    /// The recent activity, oldest first.
    pub fn activity(&self) -> Vec<(String, String, String)> {
        lock(&self.state).activity.iter().cloned().collect()
    }

    /// `time.strftime('%Y-%m-%d %H:%M')` of the injected clock.
    pub fn local_time(&self) -> String {
        (self.local_time)()
    }

    /// The pre-AI snapshot's name in the interface language: `tr('AI 작업 전 · {0}', local time)`.
    pub fn snapshot_name(&self) -> String {
        let template = grainy_i18n::tr("AI 작업 전 · {0}");
        let time = self.local_time();
        grainy_i18n::format(&template, &[time.as_str()]).unwrap_or_else(|_| template.replace("{0}", &time))
    }

    /// The next batch id token (`secrets.token_hex(3)`).
    pub fn token(&self, catalog: &Catalog) -> grainy_service::Result<String> {
        (lock(&self.tokens))(catalog)
    }

    /// `service.snapshotted`: photos that got their pre-AI snapshot in this app run.
    pub fn with_snapshotted<R>(&self, f: impl FnOnce(&mut HashSet<i64>) -> R) -> R {
        f(&mut lock(&self.state).snapshotted)
    }

    // --- jobs ------------------------------------------------------------------------------------------------

    /// `service.job`: the job that is running, if any.
    pub fn current_job(&self) -> Option<Arc<Job>> {
        lock(&self.state).job.clone()
    }

    /// `service.jobs.get(id)`.
    pub fn job(&self, id: &str) -> Option<Arc<Job>> {
        lock(&self.state).jobs.iter().find(|j| j.id == id).cloned()
    }

    /// The jobs kept for get_job, oldest first.
    pub fn jobs(&self) -> Vec<Arc<Job>> {
        lock(&self.state).jobs.iter().cloned().collect()
    }

    /// Port of `new_job`.
    pub fn new_job(&self, tool: &str, total: usize) -> Arc<Job> {
        let mut state = lock(&self.state);
        state.job_count += 1;
        let job = Arc::new(Job::new(format!("j{}", state.job_count), tool, total as i64, state.generation));
        state.jobs.push_back(job.clone());
        while state.jobs.len() > KEPT_JOBS {
            state.jobs.pop_front();
        }
        state.job = Some(job.clone());
        job
    }

    /// Port of `end_job`: marks the job finished once.
    pub fn end_job(&self, job: &Arc<Job>, state_name: &str, result: Option<Map<String, Value>>, error: Option<String>) {
        {
            let mut s = job.state();
            if s.state != "running" {
                return;
            }
            s.state = state_name.into();
            s.phase = None;
            if result.is_some() {
                s.result = result;
            }
            if error.is_some() {
                s.error = error;
            }
        }
        job.finished.notify_all();
        {
            let mut state = lock(&self.state);
            if state.job.as_ref().is_some_and(|j| Arc::ptr_eq(j, job)) {
                state.job = None;
            }
        }
        self.note(&job.tool, &format!("job {} {state_name}", job.id));
    }

    /// Port of `stale`: why a finished computation must not be written, or `None`.
    pub fn stale(&self, job: &Job) -> Option<&'static str> {
        let cancelled = job.cancel.load(Ordering::SeqCst);
        if cancelled && job.stopped() {
            return Some("stopped in Grainy; nothing was changed");
        }
        let state = lock(&self.state);
        if cancelled || job.generation != state.generation || !state.running {
            return Some("the AI connection was turned off or restarted; nothing was changed");
        }
        None
    }

    /// The stop button of the status bar (`McpWindowState.stop_job`).
    pub fn stop_job(&self) {
        if let Some(job) = self.current_job().filter(|j| j.running()) {
            job.stop();
            self.note(&job.tool, &format!("job {} stopped in Grainy", job.id));
        }
    }

    /// `McpWindowState.shutdown`: the window closes; a running job is cancelled (it wrote nothing).
    pub fn shutdown_window(&self) {
        if let Some(job) = self.current_job().filter(|j| j.running()) {
            job.cancel.store(true, Ordering::SeqCst);
            self.end_job(
                &job,
                "cancelled",
                None,
                Some("Grainy closed or reopened its window; nothing was changed".into()),
            );
        }
    }

    /// Port of `start_auto` (GUI thread, after `ready`): compute automatic edits in the window's MCP pool; they
    /// are written as one batch when done.
    #[allow(clippy::too_many_arguments)]
    pub fn start_auto(
        self: &Arc<Self>,
        host: &mut dyn Host,
        kind: grainy_service::auto::AutoKind,
        records: Vec<Value>,
        label: &str,
        tool: &str,
        amount: f64,
        extra_errors: Vec<Value>,
    ) -> grainy_service::Result<Arc<Job>> {
        let job = self.new_job(tool, records.len());
        let load = grainy_service::auto::auto_records(kind, &records)?;
        let directory = host.catalog().directory().to_path_buf();
        let worker_job = job.clone();
        let work_records = records.clone();
        let work: crate::host::Work = Box::new(move || {
            let outcome = engine::auto_outcome(kind, amount, &load, &work_records, &directory, &worker_job);
            Box::new(outcome) as Payload
        });
        let service = self.clone();
        let label = label.to_string();
        let done_job = job.clone();
        let done: crate::host::Done = Box::new(move |host, payload| {
            let outcome = *payload.downcast::<engine::AutoOutcome>().unwrap_or_else(|_| {
                Box::new(engine::AutoOutcome::Failed(
                    "could not read or compute: operation failed (internal error)".into(),
                ))
            });
            let context = AutoFinish { service, job: done_job, kind, label, extra_errors, first_try: None };
            context.finish(host, outcome);
        });
        host.spawn(work, done);
        Ok(job)
    }

    /// Port of `start_export` (GUI thread, after `ready(export=True)`): the window's export with this job's
    /// cancel and progress.
    pub fn start_export(
        self: &Arc<Self>,
        host: &mut dyn Host,
        records: Vec<Value>,
        folder: PathBuf,
        options: grainy_service::export::ExportOptions,
        extra_errors: Vec<Value>,
    ) -> grainy_service::Result<Arc<Job>> {
        let job = self.new_job("export_photos", records.len());
        let (service, done_job, folder_text) = (self.clone(), job.clone(), crate::util::path_text(&folder));
        let on_done = Box::new(move |_host: &mut dyn Host, result: Value| {
            let outputs = result.get("outputs").and_then(Value::as_array).cloned().unwrap_or_default();
            let failures = result.get("failures").and_then(Value::as_array).cloned().unwrap_or_default();
            let mut outcome = Map::new();
            outcome.insert("exported".into(), outputs.len().into());
            outcome.insert("failed".into(), failures.len().into());
            outcome.insert("skipped".into(), result.get("skipped").cloned().unwrap_or(json!(0)));
            outcome.insert("folder".into(), folder_text.into());
            outcome.insert("failures".into(), Value::Array(failures.iter().take(20).cloned().collect()));
            if let Value::Object(files) = grainy_service::export::output_files(&outputs, 100) {
                outcome.extend(files);
            }
            if failures.len() > 20 {
                outcome.insert("more_failures".into(), (failures.len() - 20).into());
            }
            if !extra_errors.is_empty() {
                outcome.extend(capped(&extra_errors));
            }
            let completion = result.get("completion_error").and_then(Value::as_str).map(String::from);
            let cancelled = done_job.cancel.load(Ordering::SeqCst);
            let state = if completion.is_some() {
                "failed"
            } else if cancelled {
                "cancelled"
            } else {
                "done"
            };
            let error = completion.or_else(|| cancelled.then(|| "stopped; files already written stay".to_string()));
            service.end_job(&done_job, state, Some(outcome), error);
        });
        let (service, failed_job) = (self.clone(), job.clone());
        let on_error = Box::new(move |_host: &mut dyn Host, last: String| {
            service.end_job(&failed_job, "failed", None, Some(format!("export failed: {last}")));
        });
        let progress_job = job.clone();
        let export = ExportStart {
            records,
            folder,
            options,
            cancel: job.cancel.clone(),
            progress: Arc::new(move |n| progress_job.set_done(n as i64)),
            on_done,
            on_error,
        };
        match host.start_export(export) {
            Err(error) => {
                self.end_job(&job, "failed", None, Some("the export could not start".into()));
                Err(error)
            }
            Ok(false) => {
                self.end_job(&job, "failed", None, Some("an export is already running".into()));
                Err(busy("an export is already running"))
            }
            Ok(true) => Ok(job),
        }
    }

    /// Port of `start_import` (GUI thread, after `ready(write=True)`): find the image files in a job, then import
    /// them with the window's queue. The job is done when none of its files is waiting any more; its result has
    /// the ids of the photos it added.
    pub fn start_import(self: &Arc<Self>, host: &mut dyn Host, folder: PathBuf, recursive: bool) -> Arc<Job> {
        let job = self.new_job("import_folder", 0);
        job.set_phase(Some("finding photos"));
        let (scan_job, own, scan_folder) = (job.clone(), self.own_folders(), folder.clone());
        let work: crate::host::Work = Box::new(move || {
            let result =
                grainy_service::export::image_files(&scan_folder, recursive, &scan_job.cancel, MAX_IMPORT, &own);
            let scanned: Result<Scanned, String> = match result {
                Ok(grainy_service::export::ImageFiles::Cancelled) => Ok(Scanned::Cancelled),
                Ok(grainy_service::export::ImageFiles::TooMany(error)) => Ok(Scanned::Error(error)),
                Ok(grainy_service::export::ImageFiles::Files(files)) => {
                    let platform = grainy_catalog::folders::Platform::native();
                    let mut seen = Vec::new();
                    let mut out = Vec::new();
                    for file in files {
                        if seen.contains(&file) {
                            continue;
                        }
                        seen.push(file.clone());
                        out.push((file.clone(), platform.path_key(&file), platform.stored_path(&file)));
                    }
                    Ok(Scanned::Files(out))
                }
                Err(error) => Err(crate::util::english(&error)),
            };
            Box::new(scanned) as Payload
        });
        let import = Arc::new(Mutex::new(Import {
            service: self.clone(),
            job: job.clone(),
            folder,
            recursive,
            first_try: None,
            files: Vec::new(),
            before: Map::new(),
            keys: Vec::new(),
        }));
        let done: crate::host::Done = Box::new(move |host, payload| {
            let scanned =
                *payload.downcast::<Result<Scanned, String>>().unwrap_or_else(|_| Box::new(Err("internal".into())));
            match scanned {
                Ok(files) => Import::guarded(&import, host, |state, host| Import::scanned(state, host, files)),
                Err(last) => {
                    let (service, job) = {
                        let state = lock(&import);
                        (state.service.clone(), state.job.clone())
                    };
                    service.end_job(&job, "failed", None, Some(format!("could not read the folder: {last}")));
                }
            }
        });
        host.spawn(work, done);
        job
    }
}

/// What a scan found (`scan()` in `start_import`).
enum Scanned {
    Cancelled,
    Error(String),
    Files(Vec<(String, String, String)>),
}

/// The state of one import job between its GUI steps (`watch` and `first_try` in the oracle).
struct Import {
    service: Arc<McpService>,
    job: Arc<Job>,
    folder: PathBuf,
    recursive: bool,
    first_try: Option<Instant>,
    files: Vec<(String, String, String)>,
    before: Map<String, Value>,
    keys: Vec<String>,
}

type Shared = Arc<Mutex<Import>>;

impl Import {
    /// `guarded(step)`: an exception ends the job as failed.
    fn guarded(shared: &Shared, host: &mut dyn Host, step: impl FnOnce(&Shared, &mut dyn Host) -> Result<(), Fault>) {
        if let Err(error) = step(shared, host) {
            let (service, job, keys) = {
                let state = lock(shared);
                (state.service.clone(), state.job.clone(), state.keys.clone())
            };
            host.release_import_quiet(&keys);
            let last = crate::util::english(&error);
            service.end_job(&job, "failed", None, Some(format!("could not finish: {last}")));
        }
    }

    /// `known(window, stored)`: `{stored path key: id}` of the originals among stored paths.
    fn known(host: &dyn Host, stored: &[String]) -> Result<Map<String, Value>, Fault> {
        let platform = grainy_catalog::folders::Platform::native();
        let mut found = Map::new();
        for part in stored.chunks(500) {
            let marks = vec!["?"; part.len()].join(",");
            let rows = host.catalog().rows(
                &format!(
                    "SELECT id,path FROM photos WHERE virtual_source IS NULL AND path COLLATE NOCASE IN ({marks})"
                ),
                grainy_catalog::rusqlite::params_from_iter(part),
            )?;
            for row in rows {
                found.insert(platform.path_key(row["path"].as_str().unwrap_or("")), row["id"].clone());
            }
        }
        Ok(found)
    }

    fn scanned(shared: &Shared, host: &mut dyn Host, files: Scanned) -> Result<(), Fault> {
        let (service, job) = {
            let state = lock(shared);
            (state.service.clone(), state.job.clone())
        };
        let files = match files {
            Scanned::Cancelled => {
                let reason = service.stale(&job).unwrap_or("stopped; nothing was imported");
                service.end_job(&job, "cancelled", None, Some(reason.into()));
                return Ok(());
            }
            Scanned::Error(error) => {
                if let Some(reason) = service.stale(&job) {
                    service.end_job(&job, "cancelled", None, Some(reason.into()));
                } else {
                    service.end_job(&job, "failed", None, Some(error));
                }
                return Ok(());
            }
            Scanned::Files(files) => files,
        };
        if let Some(reason) = service.stale(&job) {
            service.end_job(&job, "cancelled", None, Some(reason.into()));
            return Ok(());
        }
        let first = *lock(shared).first_try.get_or_insert_with(Instant::now);
        if let Err(error) = ready(host, &service, &Ready { write: true, job: Some(&job), ..Ready::default() }) {
            if first.elapsed().as_secs_f64() < service.limits().retry_seconds {
                job.set_phase(Some("waiting for Grainy"));
                let shared = shared.clone();
                host.later(
                    500,
                    Box::new(move |host| {
                        Import::guarded(&shared, host, |state, host| {
                            Import::scanned(state, host, Scanned::Files(files))
                        })
                    }),
                );
                return Ok(());
            }
            service.end_job(&job, "failed", None, Some(format!("{error}; nothing was imported. Run it again.")));
            return Ok(());
        }
        let stored: Vec<String> = files.iter().map(|(_, _, s)| s.clone()).collect();
        let before = Import::known(host, &stored)?;
        let new: Vec<(String, String)> =
            files.iter().filter(|(_, k, _)| !before.contains_key(k)).map(|(f, k, _)| (f.clone(), k.clone())).collect();
        let mut keys: Vec<String> = Vec::new();
        for (_, key) in &new {
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        let folder_text = {
            let mut state = lock(shared);
            state.files = files;
            state.before = before;
            state.keys = keys.clone();
            crate::util::path_text(&state.folder)
        };
        job.set_total(new.len() as i64);
        job.set_phase(None);
        let platform = grainy_catalog::folders::Platform::native();
        host.catalog().register_folder(&platform.stored_path(&folder_text), true)?;
        host.catalog().restore_folder(&folder_text)?;
        if new.is_empty() {
            return Import::finish(shared, host);
        }
        let files: Vec<String> = new.iter().map(|(f, _)| f.clone()).collect();
        host.queue_import_files(&files, &keys);
        Import::schedule(shared, host);
        Ok(())
    }

    /// The 250 ms polling timer: one tick later on the GUI thread.
    fn schedule(shared: &Shared, host: &mut dyn Host) {
        let shared = shared.clone();
        host.later(250, Box::new(move |host| Import::guarded(&shared, host, Import::poll)));
    }

    fn release(shared: &Shared, host: &mut dyn Host) {
        let keys = lock(shared).keys.clone();
        host.release_import_quiet(&keys);
    }

    fn poll(shared: &Shared, host: &mut dyn Host) -> Result<(), Fault> {
        let (service, job, keys) = {
            let state = lock(shared);
            (state.service.clone(), state.job.clone(), state.keys.clone())
        };
        if !job.running() || host.closing() {
            Import::release(shared, host);
            return Ok(());
        }
        if job.cancel.load(Ordering::SeqCst) {
            Import::release(shared, host);
            if job.stopped() {
                host.cancel_import();
                service.end_job(&job, "cancelled", None, Some("stopped; the photos imported so far stay".into()));
                return Ok(());
            }
            service.end_job(
                &job,
                "cancelled",
                None,
                Some("the AI connection was turned off or restarted; Grainy goes on importing".into()),
            );
            return Ok(());
        }
        let waiting = host.import_waiting(&keys);
        let scanning = host.import_scanning();
        if !scanning {
            job.set_done(job.total() - waiting as i64);
        }
        if scanning || waiting > 0 {
            Import::schedule(shared, host);
            return Ok(());
        }
        Import::finish(shared, host)
    }

    fn finish(shared: &Shared, host: &mut dyn Host) -> Result<(), Fault> {
        Import::release(shared, host);
        let (service, job, files, before, folder, recursive) = {
            let state = lock(shared);
            (
                state.service.clone(),
                state.job.clone(),
                state.files.clone(),
                state.before.clone(),
                crate::util::path_text(&state.folder),
                state.recursive,
            )
        };
        let stored: Vec<String> = files.iter().map(|(_, _, s)| s.clone()).collect();
        let after = Import::known(host, &stored)?;
        let ids: Vec<Value> = files
            .iter()
            .filter(|(_, k, _)| after.contains_key(k) && !before.contains_key(k))
            .map(|(_, k, _)| after[k].clone())
            .collect();
        let missing: Vec<Value> = files
            .iter()
            .filter(|(_, k, _)| !after.contains_key(k))
            .map(|(f, _, _)| Value::String(crate::util::file_name(f)))
            .collect();
        let mut result = Map::new();
        result.insert("imported".into(), ids.len().into());
        result.insert("ids".into(), Value::Array(ids.iter().take(MAX_REPORTED).cloned().collect()));
        result.insert("already".into(), before.len().into());
        result.insert("folder".into(), folder.into());
        result.insert("recursive".into(), recursive.into());
        if ids.len() > MAX_REPORTED {
            result.insert("more_ids".into(), (ids.len() - MAX_REPORTED).into());
        }
        if !missing.is_empty() {
            result.insert("not_imported".into(), missing.len().into());
            result.insert("failures".into(), Value::Array(missing.iter().take(20).cloned().collect()));
        }
        let keys: Vec<String> = files.iter().map(|(_, k, _)| k.clone()).collect();
        let completion = host.take_import_completion_errors(&keys);
        if completion.is_empty() {
            service.end_job(&job, "done", Some(result), None);
        } else {
            service.end_job(&job, "failed", Some(result), Some("could not save imported photos to the catalog".into()));
        }
        Ok(())
    }
}

/// The GUI half of `start_auto` (`finish` and `guarded`).
struct AutoFinish {
    service: Arc<McpService>,
    job: Arc<Job>,
    kind: grainy_service::auto::AutoKind,
    label: String,
    extra_errors: Vec<Value>,
    first_try: Option<Instant>,
}

impl AutoFinish {
    fn finish(mut self, host: &mut dyn Host, outcome: engine::AutoOutcome) {
        let (service, job) = (self.service.clone(), self.job.clone());
        let outcome = match outcome {
            engine::AutoOutcome::Failed(error) => {
                service.end_job(&job, "failed", None, Some(error));
                return;
            }
            other => other,
        };
        let reason = match &outcome {
            engine::AutoOutcome::Cancelled => Some(service.stale(&job).unwrap_or("stopped; nothing was changed")),
            _ => service.stale(&job),
        };
        if let Some(reason) = reason {
            service.end_job(&job, "cancelled", None, Some(reason.into()));
            return;
        }
        let (results, errors) = match outcome {
            engine::AutoOutcome::Error { error, .. } => {
                service.end_job(&job, "failed", None, Some(error));
                return;
            }
            engine::AutoOutcome::Results { results, errors } => (results, errors),
            _ => return,
        };
        let ids: Vec<i64> = results.iter().map(|(r, _, _)| crate::util::id_of(r)).collect();
        let first = *self.first_try.get_or_insert_with(Instant::now);
        let check = ready(host, &service, &Ready { write: true, ids: &ids, job: Some(&job), ..Ready::default() });
        if let Err(error) = check {
            if first.elapsed().as_secs_f64() < service.limits().retry_seconds {
                job.set_phase(Some("waiting for Grainy"));
                let outcome = engine::AutoOutcome::Results { results, errors };
                host.later(500, Box::new(move |host| self.finish(host, outcome)));
                return;
            }
            service.end_job(&job, "failed", None, Some(format!("{error}; nothing was changed. Run it again.")));
            return;
        }
        if let Err(error) = self.write(host, results, errors) {
            let last = crate::util::english(&error);
            service.end_job(&job, "failed", None, Some(format!("could not finish: {last}")));
        }
    }

    fn write(
        &self,
        host: &mut dyn Host,
        results: Vec<(Value, Value, String)>,
        errors: Vec<Value>,
    ) -> Result<(), Fault> {
        let ids: Vec<i64> = results.iter().map(|(r, _, _)| crate::util::id_of(r)).collect();
        if host.current_id().is_some_and(|c| ids.contains(&c)) {
            host.commit()?;
            host.save_keywords()?;
        }
        let (mut used, mut values, mut meanwhile) = (Vec::new(), Vec::new(), Vec::new());
        let mut statuses: Vec<(String, Vec<Value>)> = Vec::new();
        for (record, settings, status) in results {
            let ident = crate::util::id_of(&record);
            let now = host.catalog().photo(ident)?;
            let same = now.as_ref().is_some_and(|n| {
                eq(get(n, "settings"), get(&record, "settings")) && get(n, "path") == get(&record, "path")
            });
            if !same {
                meanwhile.push(Value::from(ident));
                continue;
            }
            used.push(record);
            values.push(settings);
            match statuses.iter_mut().find(|(s, _)| *s == status) {
                Some((_, list)) => list.push(ident.into()),
                None => statuses.push((status, vec![ident.into()])),
            }
        }
        let (bid, changed) = if used.is_empty() {
            (None, Vec::new())
        } else {
            write_batch_raw(host, &self.service, &used, &values, &self.label, &self.job.tool, None)?
        };
        if !changed.is_empty() {
            host.refresh_command_edits(&changed, &self.label);
        }
        let mut result = Map::new();
        result.insert("batch".into(), bid.map_or(Value::Null, Value::String));
        result.insert("changed".into(), changed.len().into());
        result.insert("unchanged".into(), (used.len() - changed.len()).into());
        let status = |name: &str| {
            Value::Array(statuses.iter().find(|(s, _)| s == name).map(|(_, l)| l.clone()).unwrap_or_default())
        };
        match self.kind {
            grainy_service::auto::AutoKind::Tone => {
                result.insert("flat".into(), status("flat"));
            }
            grainy_service::auto::AutoKind::Color => {
                let mut skipped = Map::new();
                for key in ["neutral", "monochrome", "flat"] {
                    skipped.insert(key.into(), status(key));
                }
                result.insert("skipped".into(), Value::Object(skipped));
            }
            _ => {}
        }
        result.insert("edited_meanwhile".into(), Value::Array(meanwhile));
        let mut all = self.extra_errors.clone();
        all.extend(errors);
        result.extend(capped(&all));
        self.service.end_job(&self.job, "done", Some(result), None);
        Ok(())
    }
}

/// `photo_service.apply_batch` with this service's snapshot name, snapshotted set and batch ids (GUI thread).
pub(crate) fn write_batch_raw(
    host: &mut dyn Host,
    service: &McpService,
    used: &[Value],
    values: &[Value],
    label: &str,
    tool: &str,
    presets: Option<&std::collections::BTreeMap<i64, String>>,
) -> Result<(Option<String>, Vec<i64>), Fault> {
    let snapshot_name = service.snapshot_name();
    let options =
        grainy_service::stack::BatchOptions { label, tool, snapshot_name: &snapshot_name, switch: false, presets };
    let catalog = host.catalog();
    let mut snapshotted = service.with_snapshotted(|s| s.clone());
    let mut tokens = || service.token(catalog);
    let result = grainy_service::stack::apply_batch(catalog, used, values, &options, &mut snapshotted, &mut tokens)?;
    service.with_snapshotted(|s| *s = snapshotted);
    Ok(result)
}

/// The arguments of [`ready`].
#[derive(Default)]
pub struct Ready<'a> {
    /// The request writes the catalogue.
    pub write: bool,
    /// The request exports.
    pub export: bool,
    /// The photos it writes (the open one among them needs the editor to be idle).
    pub ids: &'a [i64],
    /// It targets the selection (needs the photo list loaded).
    pub selection: bool,
    /// The MCP job that is finishing (it does not count as running).
    pub job: Option<&'a Arc<Job>>,
}

/// Port of `ready`: `Busy` (English reason) unless Grainy can take this request now. GUI thread; nothing is
/// written before this passes.
pub fn ready(host: &dyn Host, service: &McpService, r: &Ready<'_>) -> Result<(), Fault> {
    if host.closing() {
        return Err(busy("Grainy is closing or reopening its window"));
    }
    if !(r.write || r.export) {
        return Ok(());
    }
    if host.modal_open() {
        return Err(busy("a dialog or menu is open in Grainy; ask the person to close it"));
    }
    if grainy_service::batch::in_transaction(host.catalog()) {
        return Err(busy("Grainy is saving something else"));
    }
    if host.command_running() {
        return Err(busy("a photo batch is running"));
    }
    if host.maintenance_running() {
        return Err(busy("a catalog task is running"));
    }
    if host.importing() {
        return Err(busy("photos are being imported"));
    }
    if r.export && host.export_running() {
        return Err(busy("an export is already running"));
    }
    if let Some(running) = service.current_job() {
        let mine = r.job.is_some_and(|j| Arc::ptr_eq(j, &running));
        if running.running() && !mine {
            return Err(busy(format!("job {} ({}) is still running; call get_job", running.id, running.tool)));
        }
    }
    if r.write && host.current_id().is_some_and(|c| r.ids.contains(&c)) {
        if host.interacting() {
            return Err(busy("the person is adjusting the open photo"));
        }
        if host.unfinished_marks() {
            return Err(busy("the open photo has unfinished brush or healing marks"));
        }
        if host.tool_open() {
            return Err(busy("a crop or picker tool is open on the open photo"));
        }
    }
    if r.write && r.selection && host.library_loading() {
        return Err(busy("the photo list is still loading"));
    }
    Ok(())
}

/// Port of `safe_commit`: save the open photo's pending change when nothing is in the middle of it. True when
/// saved (or nothing was pending); false leaves the caller with the last saved state.
pub fn safe_commit(host: &mut dyn Host) -> Result<bool, Fault> {
    if !host.unsaved() {
        return Ok(true);
    }
    if host.interacting() || host.nested_loop() || grainy_service::batch::in_transaction(host.catalog()) {
        return Ok(false);
    }
    host.commit()?;
    Ok(true)
}
