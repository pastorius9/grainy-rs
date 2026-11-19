//! Port of `ai_programs.AiRunner`: one AI program run on its own thread, its JSON event stream decoded into the
//! window's events (secrets redacted), its stderr kept in memory only (8 KiB, for the sign-in check), a time limit,
//! and one fixed result. Raw diagnostics are never shown or stored.

use std::io::{BufRead, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::auth::authentication_failed;
use super::programs::{redact, Event, EventParser, Profile, Request};
use super::system::{Launch, System};

/// What the run thread tells the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunnerMessage {
    /// `event` signal.
    Event(Event),
    /// `finished` signal: `done`, `stopped`, `timeout`, `auth_required` or `failed`.
    Finished(String),
}

/// `AiRunner.start` refused (`RuntimeError`), or the arguments could not be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartError(pub String);

/// The shared state of one run (the oracle's `AiRunner` attributes the thread writes).
#[derive(Default)]
struct Shared {
    session_id: Mutex<String>,
    diagnostic: Mutex<String>,
    stopper: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

/// Port of `AiRunner`.
pub struct AiRunner {
    sys: Arc<dyn System>,
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    shared: Arc<Shared>,
    tx: Sender<RunnerMessage>,
    rx: Receiver<RunnerMessage>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The time limit of a run (`timeout=600`).
    pub timeout: Duration,
}

impl AiRunner {
    /// A runner starting programs through `sys`.
    pub fn new(sys: Arc<dyn System>) -> Self {
        let (tx, rx) = channel();
        Self {
            sys,
            running: Arc::new(AtomicBool::new(false)),
            cancel: Arc::new(AtomicBool::new(false)),
            shared: Arc::new(Shared::default()),
            tx,
            rx,
            thread: None,
            timeout: Duration::from_secs(600),
        }
    }

    /// `runner.running`.
    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// `runner.session_id`: the program's session, from its stream.
    pub fn session_id(&self) -> String {
        self.shared.session_id.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// `runner.session_id = ''` (a new conversation).
    pub fn clear_session(&self) {
        if let Ok(mut s) = self.shared.session_id.lock() {
            s.clear();
        }
    }

    /// `runner.diagnostic`: `authentication required`, `program failed` or empty.
    pub fn diagnostic(&self) -> String {
        self.shared.diagnostic.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Port of `start(profile, request, secrets, timeout)`: the workspace is made, the program's arguments built
    /// (its run files written), and the run starts on its own thread.
    pub fn start(&mut self, profile: &Profile, request: &Request, secrets: &[String]) -> Result<(), StartError> {
        if self.running() {
            return Err(StartError("AI is already running".into()));
        }
        std::fs::create_dir_all(&request.workspace).map_err(|e| StartError(e.to_string()))?;
        let argv = (profile.build)(request, self.sys.as_ref()).map_err(|e| StartError(e.to_string()))?;
        if let Ok(mut d) = self.shared.diagnostic.lock() {
            d.clear();
        }
        self.cancel.store(false, Ordering::SeqCst);
        self.running.store(true, Ordering::SeqCst);
        let (sys, running, cancel, shared, tx) =
            (self.sys.clone(), self.running.clone(), self.cancel.clone(), self.shared.clone(), self.tx.clone());
        let (profile, request, secrets, timeout) = (*profile, request.clone(), secrets.to_vec(), self.timeout);
        self.thread = Some(std::thread::spawn(move || {
            let mut emit = |event: Event| {
                let _ = tx.send(RunnerMessage::Event(event));
            };
            let result = run(sys.as_ref(), &profile, &request, argv, &secrets, timeout, &cancel, &shared, &mut emit);
            running.store(false, Ordering::SeqCst);
            let _ = tx.send(RunnerMessage::Finished(result));
        }));
        Ok(())
    }

    /// The events and the end of the run since the last call (the window's signal slots).
    pub fn poll(&self) -> Vec<RunnerMessage> {
        self.rx.try_iter().collect()
    }

    /// Port of `stop()`.
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(stop) = self.shared.stopper.lock().ok().and_then(|s| s.clone()) {
            stop();
        }
    }

    /// Port of `shutdown()`: stop, and wait up to 12 seconds for the run thread.
    pub fn shutdown(&mut self) {
        self.stop();
        if let Some(thread) = self.thread.take() {
            let until = std::time::Instant::now() + Duration::from_secs(12);
            while !thread.is_finished() && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

/// The environment's own secrets: values of 8 characters or more of variables named `…API_KEY`, `…TOKEN`,
/// `…SECRET`, `…PASSWORD`.
fn environment_secrets(env: &[(String, String)]) -> Vec<String> {
    env.iter()
        .filter(|(k, v)| {
            let upper = k.to_uppercase();
            v.chars().count() >= 8 && ["API_KEY", "TOKEN", "SECRET", "PASSWORD"].iter().any(|s| upper.ends_with(s))
        })
        .map(|(_, v)| v.clone())
        .collect()
}

fn set_env(env: &mut Vec<(String, String)>, key: &str, value: String, only_missing: bool) {
    match env.iter_mut().find(|(k, _)| k == key) {
        Some(slot) if !only_missing => slot.1 = value,
        Some(_) => {}
        None => env.push((key.into(), value)),
    }
}

/// Port of `AiRunner._run` (synchronous: the thread body). Returns the result; events go to `emit`.
#[allow(clippy::too_many_arguments)]
fn run(
    sys: &dyn System,
    profile: &Profile,
    request: &Request,
    argv: Vec<String>,
    secrets: &[String],
    timeout: Duration,
    cancel: &Arc<AtomicBool>,
    shared: &Arc<Shared>,
    emit: &mut dyn FnMut(Event),
) -> String {
    let mut env = sys.vars();
    let mut secrets = secrets.to_vec();
    secrets.extend(environment_secrets(&env));
    if profile.key == "claude" {
        set_env(
            &mut env,
            "CLAUDE_CODE_DEBUG_LOGS_DIR",
            request.workspace.join("claude-log").to_string_lossy().into(),
            false,
        );
        // Wait for Grainy's server before the first turn.
        set_env(&mut env, "MCP_CONNECTION_NONBLOCKING", "0".into(), false);
        set_env(&mut env, "MCP_CONNECT_TIMEOUT_MS", "30000".into(), true);
    }
    if profile.key == "gemini" {
        // A per-process system-scope overlay wins even over a user's tools.core list.
        let path = request.workspace.join(".gemini").join("settings.json");
        set_env(&mut env, "GEMINI_CLI_SYSTEM_SETTINGS_PATH", path.to_string_lossy().into(), false);
    }
    let launch = Launch { argv, cwd: request.workspace.clone(), env: Some(env) };
    let mut process = match sys.piped(&launch) {
        Ok(p) => p,
        Err(_) => {
            emit(Event::new("launch_error", "", None));
            return "failed".into();
        }
    };
    let stop = process.stopper();
    if let Ok(mut slot) = shared.stopper.lock() {
        *slot = Some(stop.clone());
    }
    if cancel.load(Ordering::SeqCst) {
        stop();
    }
    let auth_failed = Arc::new(AtomicBool::new(false));
    let reader = process.stderr().map(|mut stderr| {
        let auth_failed = auth_failed.clone();
        std::thread::spawn(move || {
            // Bounded, in memory only; never shown.
            let mut kept: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = stderr.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                kept.extend_from_slice(&chunk[..n]);
                let excess = kept.len().saturating_sub(8192);
                kept.drain(..excess);
                if authentication_failed(&String::from_utf8_lossy(&kept)) {
                    auth_failed.store(true, Ordering::SeqCst);
                }
            }
        })
    });
    let timed_out = Arc::new(AtomicBool::new(false));
    let (timer_tx, timer_rx) = channel::<()>();
    let timer = {
        let (timed_out, stop) = (timed_out.clone(), stop.clone());
        std::thread::spawn(move || {
            if let Err(std::sync::mpsc::RecvTimeoutError::Timeout) = timer_rx.recv_timeout(timeout) {
                timed_out.store(true, Ordering::SeqCst);
                stop();
            }
        })
    };
    let mut parser = EventParser::new();
    if let Some(mut stdout) = process.stdout() {
        loop {
            let mut line = Vec::new();
            match (&mut stdout).take(4 * 1024 * 1024).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let decoded =
                std::str::from_utf8(&line).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok());
            let events = match decoded {
                Some(data) if data.is_object() => (profile.parse)(&mut parser, &data).ok(),
                Some(_) => continue,
                None => None,
            };
            let Some(events) = events else {
                if authentication_failed(&String::from_utf8_lossy(&line)) {
                    auth_failed.store(true, Ordering::SeqCst);
                }
                continue;
            };
            for event in events {
                if event.kind == "session" {
                    if let Ok(mut s) = shared.session_id.lock() {
                        *s = event.text.clone();
                    }
                }
                if event.kind == "error" || event.kind == "auth_error" {
                    parser.failed = true;
                    if event.kind == "auth_error" {
                        auth_failed.store(true, Ordering::SeqCst);
                    }
                    // A single fixed failure is told after the program ends.
                    continue;
                }
                emit(Event::new(&event.kind, redact(&event.text, &secrets), event.count));
            }
        }
    }
    let code = process.wait().unwrap_or(-1);
    stop(); // a bridge or launcher that outlived its parent
    if let Some(reader) = reader {
        let until = std::time::Instant::now() + Duration::from_secs(2);
        while !reader.is_finished() && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let _ = timer_tx.send(());
    let _ = timer.join();
    if let Ok(mut slot) = shared.stopper.lock() {
        *slot = None;
    }
    let set_diagnostic = |text: &str| {
        if let Ok(mut d) = shared.diagnostic.lock() {
            *d = text.into();
        }
    };
    let auth = auth_failed.load(Ordering::SeqCst);
    if cancel.load(Ordering::SeqCst) {
        "stopped".into()
    } else if timed_out.load(Ordering::SeqCst) {
        "timeout".into()
    } else if (code != 0 || parser.failed || !parser.succeeded) && (auth || profile.auth_exit_codes.contains(&code)) {
        set_diagnostic("authentication required");
        emit(Event::new("auth_error", "", None));
        "auth_required".into()
    } else if code == 0 && parser.succeeded && !parser.failed {
        "done".into()
    } else {
        // Diagnostics can hold credentials or whole HTTP requests: neither is stored or shown.
        set_diagnostic("program failed");
        emit(Event::new("exit", code.to_string(), None));
        "failed".into()
    }
}

/// [`run`] for tests and goldens: one run on this thread with the given secrets; the events, the result, the
/// session id and the diagnostic.
pub fn run_here(
    sys: &dyn System,
    profile: &Profile,
    request: &Request,
    argv: Vec<String>,
    secrets: &[String],
    timeout: Duration,
) -> (Vec<Event>, String, String, String) {
    let shared = Arc::new(Shared::default());
    let cancel = Arc::new(AtomicBool::new(false));
    let mut events = Vec::new();
    let result = run(sys, profile, request, argv, secrets, timeout, &cancel, &shared, &mut |e| events.push(e));
    let session = shared.session_id.lock().map(|s| s.clone()).unwrap_or_default();
    let diagnostic = shared.diagnostic.lock().map(|s| s.clone()).unwrap_or_default();
    (events, result, session, diagnostic)
}
