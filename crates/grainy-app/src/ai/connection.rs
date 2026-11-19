//! The AI connection's lifetime in the window (port of the server half of luma/mcp_service.py `McpService`: the
//! switch and settings, start / stop / other port / new key, the owner lock and the files the stdio bridges read)
//! and the texts of Settings › AI Connection that do not depend on a window (luma/mcp_dialog.py `install_commands`,
//! `install_text`, `standard_folder`, `activity_text`).
//!
//! The tools themselves are grainy-mcp's ([`GrainyTools`] on an [`McpService`]); they reach the window through a
//! [`GuiQueue`] that the window pumps every frame with its [`super::host::AppHost`]. The HTTP server is started
//! through a [`Binder`], keys and ports come from [`Sources`]: tests give fakes, so no socket is opened and nothing
//! random or timed shows in their results.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use grainy_mcp::bridge::{self, Bridge};
use grainy_mcp::gui::GuiQueue;
use grainy_mcp::http::Endpoint;
use grainy_mcp::service::{Config, McpService, DEFAULT_PORT};
use grainy_mcp::{GrainyTools, Gui, Protocol};
use grainy_platform::json::Value as Json;
use grainy_platform::Preferences;
use serde_json::{json, Value};

use super::system::System;
use crate::text::{tr, tr_with};

/// `WINDOWS_BRIDGE`: the bridge's place on every Windows computer, as PowerShell expands it.
pub const WINDOWS_BRIDGE: &str = "$env:LOCALAPPDATA\\Grainy\\mcp\\grainy-mcp-bridge.ps1";
/// `MACOS_BRIDGE`.
pub const MACOS_BRIDGE: &str = "$HOME/Library/Application Support/Grainy/mcp/grainy-mcp-bridge.sh";
/// `CLIENTS`: the install tabs (key, title).
pub const CLIENTS: [(&str, &str); 3] =
    [("claude_code", "Claude Code"), ("codex", "Codex"), ("claude_desktop_json", "Claude 데스크톱")];
/// The stdio bridges the AI programs start (the oracle's `assets/mcp`), shipped inside the program.
pub const BRIDGES: [(&str, &[u8]); 2] = [
    ("grainy-mcp-bridge.ps1", include_bytes!("../../assets/mcp/grainy-mcp-bridge.ps1")),
    ("grainy-mcp-bridge.sh", include_bytes!("../../assets/mcp/grainy-mcp-bridge.sh")),
];
/// `TOKEN_PATTERN`: 64 lower-case hex digits.
fn is_token(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Port of `standard_folder(windows)`: where Grainy keeps the bridges for this user, the folder the install
/// commands name without spelling it out.
pub fn standard_folder(sys: &dyn System, windows: bool) -> Option<PathBuf> {
    if windows {
        return sys
            .var("LOCALAPPDATA")
            .filter(|v| !v.is_empty())
            .map(|base| PathBuf::from(base).join("Grainy").join("mcp"));
    }
    (sys.os() == super::system::Os::MacOs)
        .then(|| sys.home().join("Library").join("Application Support").join("Grainy").join("mcp"))
}

/// `os.path.normcase(os.path.abspath(path))`.
fn normcase(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let text = absolute.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.replace('/', "\\").to_lowercase()
    } else {
        text
    }
}

/// Port of `same_folder(a, b)`.
pub fn same_folder(a: &Path, b: Option<&Path>) -> bool {
    b.is_some_and(|b| normcase(a) == normcase(b))
}

/// Port of `install_commands(windows, folder)`: what the person runs once (PowerShell or Terminal) or puts into
/// `claude_desktop_config.json`; never the key or the port. `standard` is [`standard_folder`] of this computer:
/// a `folder` equal to it is not spelled out.
pub fn install_commands(windows: bool, folder: Option<&Path>, standard: Option<&Path>) -> Value {
    let folder = folder.filter(|f| !same_folder(f, standard));
    let (run, server) = if windows {
        let (path, start) = match folder {
            None => (
                format!("\"{WINDOWS_BRIDGE}\""),
                // JSON cannot expand %LOCALAPPDATA%: PowerShell finds the folder when Claude Desktop starts it.
                json!(["-Command", "& (Join-Path $env:LOCALAPPDATA 'Grainy\\mcp\\grainy-mcp-bridge.ps1')"]),
            ),
            Some(folder) => {
                let script = host_text(&folder.join("grainy-mcp-bridge.ps1"));
                (format!("'{}'", script.replace('\'', "''")), json!(["-File", script]))
            }
        };
        // '--' quoted: PowerShell drops a bare -- before a .ps1 script, and npm installs claude and codex as scripts.
        let run = format!("'--' powershell -NoProfile -ExecutionPolicy Bypass -File {path}");
        let mut args = vec![json!("-NoProfile"), json!("-ExecutionPolicy"), json!("Bypass")];
        args.extend(start.as_array().cloned().unwrap_or_default());
        (run, json!({"command": "powershell", "args": args}))
    } else {
        let (path, server) = match folder {
            None => {
                let path = format!("\"{MACOS_BRIDGE}\"");
                let server = json!({"command": "/bin/sh", "args": ["-c", format!("exec /bin/sh {path}")]});
                (path, server)
            }
            Some(folder) => {
                let script = host_text(&folder.join("grainy-mcp-bridge.sh"));
                (format!("'{}'", script.replace('\'', "'\\''")), json!({"command": "/bin/sh", "args": [script]}))
            }
        };
        (format!("-- sh {path}"), server)
    };
    json!({
        "claude_code": format!("claude mcp add --scope user grainy {run}"),
        "codex": format!("codex mcp add grainy {run}"),
        "claude_desktop_json": {"mcpServers": {"grainy": server}},
    })
}

/// `str(path)` of the oracle's `Path`: on Windows every `/` reads `\`.
fn host_text(path: &Path) -> String {
    let text = path.to_string_lossy().into_owned();
    if cfg!(windows) {
        text.replace('/', "\\")
    } else {
        text
    }
}

/// Port of `install_text(value)`: a command as it is, the JSON as `json.dumps(indent=2, ensure_ascii=False)`.
pub fn install_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => super::programs::py_dumps(other, Some(2), false),
    }
}

/// Port of `activity_text(name, outcome)`: one line of the recent activity in the interface language.
pub fn activity_text(name: &str, outcome: &str) -> String {
    let name = match name {
        "server" => tr("AI 연결"),
        "files" => tr("중계 파일"),
        "key" => tr("연결 키"),
        other => other.to_owned(),
    };
    let fixed = match outcome {
        "started" => Some("시작함"),
        "stopped" => Some("멈춤"),
        "could not start" => Some("시작하지 못함"),
        "port not saved" => Some("포트를 저장하지 못함"),
        "on in another Grainy" => Some("다른 Grainy에서 켜져 있음"),
        "new key" => Some("새 키를 만듦"),
        "could not write the bridge files" => Some("쓰지 못함 (설치한 AI 프로그램이 연결할 수 없음)"),
        "refused 401" => Some("거부함: 연결 키가 없거나 맞지 않음"),
        "refused 403" => Some("거부함: 허용되지 않은 주소나 웹 페이지에서 온 요청"),
        "ok" => Some("성공"),
        "error" => Some("오류"),
        "undone in Grainy" => Some("Grainy에서 되돌림"),
        _ => None,
    };
    let text = fixed.map(tr).or_else(|| job_outcome(outcome)).or_else(|| code_outcome(outcome)).or_else(|| {
        let (head, rest) = outcome.split_once(": ")?;
        match head {
            "error" => Some(tr_with("오류: {0}", &[rest])),
            "failed" => Some(tr_with("실패: {0}", &[rest])),
            _ => None,
        }
    });
    format!("{name} · {}", text.unwrap_or_else(|| outcome.to_owned()))
}

/// `job (j\d+) (done|failed|cancelled|stopped in Grainy)`.
fn job_outcome(outcome: &str) -> Option<String> {
    let rest = outcome.strip_prefix("job j")?;
    let (digits, state) = rest.split_once(' ')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let job = format!("j{digits}");
    let source = match state {
        "done" => "작업 {0} 완료",
        "failed" => "작업 {0} 실패",
        "cancelled" => "작업 {0} 취소됨",
        "stopped in Grainy" => "작업 {0}을(를) Grainy에서 멈춤",
        _ => return None,
    };
    Some(tr_with(source, &[&job]))
}

/// `(refused|error) (-?\d+)`.
fn code_outcome(outcome: &str) -> Option<String> {
    let (head, code) = outcome.split_once(' ')?;
    let digits = code.strip_prefix('-').unwrap_or(code);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    match head {
        "refused" => Some(tr_with("거부함 ({0})", &[code])),
        "error" => Some(tr_with("오류 {0}", &[code])),
        _ => None,
    }
}

/// A running HTTP server.
pub trait Bound: Send {
    /// The port it listens on.
    fn port(&self) -> u16;
    /// Stops it (waits briefly for requests in flight).
    fn stop(self: Box<Self>);
}

/// Starts the HTTP server (`mcp_http.start(port, service, gui)`).
pub trait Binder: Send {
    /// Binds 127.0.0.1:`port` and serves `endpoint`.
    fn bind(&mut self, port: u16, endpoint: Endpoint) -> std::io::Result<Box<dyn Bound>>;
}

/// The real server of grainy-mcp, on 127.0.0.1 only.
pub struct HttpBinder;

impl Bound for grainy_mcp::http::Server {
    fn port(&self) -> u16 {
        grainy_mcp::http::Server::port(self)
    }
    fn stop(self: Box<Self>) {
        grainy_mcp::http::Server::stop(*self);
    }
}

impl Binder for HttpBinder {
    fn bind(&mut self, port: u16, endpoint: Endpoint) -> std::io::Result<Box<dyn Bound>> {
        super::system::require_online()?;
        Ok(Box::new(grainy_mcp::http::start(port, endpoint)?))
    }
}

/// `secrets.token_hex(n)` from SQLite's cryptographically strong generator (no new dependency).
pub fn token_hex(bytes: usize) -> String {
    let random = || -> Option<String> {
        let db = grainy_catalog::rusqlite::Connection::open_in_memory().ok()?;
        db.query_row(&format!("SELECT lower(hex(randomblob({bytes})))"), [], |r| r.get::<_, String>(0)).ok()
    };
    random().unwrap_or_else(|| {
        // Never reached in practice: an in-memory database always opens.
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        format!("{nanos:0width$x}", width = bytes * 2).chars().take(bytes * 2).collect()
    })
}

/// `random.randint(40000, 48999)`.
pub fn random_port() -> u16 {
    let value = u64::from_str_radix(&token_hex(4), 16).unwrap_or(0);
    40000 + (value % 9000) as u16
}

/// Where keys, ports and activity times come from.
pub struct Sources {
    /// `secrets.token_hex(32)`.
    pub token: Box<dyn FnMut() -> String + Send>,
    /// `random.randint(40000, 48999)`.
    pub port: Box<dyn FnMut() -> u16 + Send>,
    /// The service's clock (`time.strftime`) and batch ids, or `None` for the real ones.
    pub service: Option<(grainy_mcp::service::TokenSource, Box<dyn Fn() -> String + Send + Sync>)>,
}

impl Default for Sources {
    fn default() -> Self {
        Self { token: Box::new(|| token_hex(32)), port: Box::new(random_port), service: None }
    }
}

/// What a [`Connection`] is made for.
pub struct Setup {
    /// The settings folder (`Path(preferences.path).parent`): `settings.json` and the `mcp` folder.
    pub settings_folder: PathBuf,
    /// The library folder the tools work on.
    pub library: PathBuf,
    /// `pictures_folder()` (tests give their own).
    pub pictures: PathBuf,
    /// The version the server reports.
    pub version: String,
    /// Starts the HTTP server.
    pub binder: Box<dyn Binder>,
    /// Keys, ports, clocks.
    pub sources: Sources,
    /// Called when a request waits for the window (it repaints).
    pub wake: Arc<dyn Fn() + Send + Sync>,
}

impl Setup {
    /// The real setup for a library and a settings folder.
    pub fn new(settings_folder: PathBuf, library: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self {
            settings_folder,
            library,
            pictures: grainy_mcp::service::default_pictures(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            binder: Box::new(HttpBinder),
            sources: Sources::default(),
            wake,
        }
    }
}

/// Port of `McpService`'s connection state: one per window and library.
pub struct Connection {
    preferences: Preferences,
    settings_folder: PathBuf,
    library: PathBuf,
    /// The tools' service (jobs, activity, limits).
    pub service: Arc<McpService>,
    bridge: Bridge,
    server: Option<Box<dyn Bound>>,
    gui: Option<Arc<GuiQueue>>,
    /// `running`.
    pub running: bool,
    /// `error`: why the connection is not on, in the interface language.
    pub error: Option<String>,
    /// `elsewhere`: another Grainy on this computer has it on.
    pub elsewhere: bool,
    /// `port_changed`: Use Another Port was used.
    pub port_changed: bool,
    binder: Box<dyn Binder>,
    tokens: Box<dyn FnMut() -> String + Send>,
    ports: Box<dyn FnMut() -> u16 + Send>,
    version: String,
    wake: Arc<dyn Fn() + Send + Sync>,
}

/// An `OSError`'s `strerror`: the message without Rust's " (os error N)".
fn strerror(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.rfind(" (os error ") {
        Some(at) if text.ends_with(')') => text[..at].to_owned(),
        _ => text,
    }
}

impl Connection {
    /// The connection of `setup` (not started: [`Connection::start_if_enabled`]).
    pub fn new(setup: Setup) -> Self {
        let preferences = Preferences::new(setup.settings_folder.join("settings.json"));
        let mut config = Config::new(setup.library.clone(), setup.settings_folder.clone());
        config.program_folders.extend(crate::paths::Locations::current().oracle_folders());
        config.pictures = setup.pictures;
        let Sources { token, port, service } = setup.sources;
        let service = match service {
            Some((tokens, clock)) => McpService::with_sources(config, tokens, clock),
            None => McpService::new(config),
        };
        let connection = Self {
            preferences,
            bridge: Bridge::new(&setup.settings_folder),
            settings_folder: setup.settings_folder,
            library: setup.library,
            service,
            server: None,
            gui: None,
            running: false,
            error: None,
            elsewhere: false,
            port_changed: false,
            binder: setup.binder,
            tokens: token,
            ports: port,
            version: setup.version,
            wake: setup.wake,
        };
        connection.sync_service();
        connection
    }

    /// The library the tools work on.
    pub fn library(&self) -> &Path {
        &self.library
    }

    /// The settings folder.
    pub fn settings_folder(&self) -> &Path {
        &self.settings_folder
    }

    /// The settings file's values (`preferences.values`).
    pub fn preferences(&self) -> &Preferences {
        &self.preferences
    }

    /// `preferences.save(key, value)`; the tools see the new values.
    pub fn save(&mut self, key: &str, value: Json) -> std::io::Result<()> {
        self.preferences.save(key, value)?;
        self.sync_service();
        Ok(())
    }

    fn sync_service(&self) {
        let value = match self.preferences.values.get("mcp_export_folder") {
            Some(Json::Str(s)) => json!(s),
            Some(other) => serde_json::from_str(&other.dump(None, false)).unwrap_or(Value::Null),
            None => Value::Null,
        };
        self.service.set_preference("mcp_export_folder", value);
    }

    /// `McpService.folder`: the bridges' folder.
    pub fn folder(&self) -> &Path {
        self.bridge.folder()
    }

    /// `enabled`: on unless the person switched it off.
    pub fn enabled(&self) -> bool {
        !matches!(self.preferences.values.get("mcp_enabled"), Some(Json::Bool(false)))
    }

    /// `port`: the saved port when it is a usable one.
    pub fn port(&self) -> Option<u16> {
        match self.preferences.values.get("mcp_port") {
            Some(Json::Int(v)) if (1024..=65535).contains(v) => Some(*v as u16),
            _ => None,
        }
    }

    /// `token`: the saved key when it has the right form.
    pub fn token(&self) -> Option<String> {
        match self.preferences.values.get("mcp_token") {
            Some(Json::Str(v)) if is_token(v) => Some(v.clone()),
            _ => None,
        }
    }

    /// `bound_port`.
    pub fn bound_port(&self) -> Option<u16> {
        self.server.as_ref().map(|s| s.port())
    }

    /// Port of `export_root`: the AI export folder (shown only; the tools check it themselves).
    pub fn export_root(&self) -> PathBuf {
        match self.preferences.values.get("mcp_export_folder") {
            Some(Json::Str(s)) if !s.is_empty() => PathBuf::from(s),
            _ => self.service.config().pictures.clone(),
        }
    }

    /// The window's call queue of the running server.
    pub fn gui(&self) -> Option<Arc<GuiQueue>> {
        self.gui.clone()
    }

    fn note(&self, name: &str, outcome: &str) {
        self.service.note(name, outcome);
    }

    fn ensure_secrets(&mut self) -> std::io::Result<()> {
        if self.token().is_none() {
            let token = (self.tokens)();
            self.save("mcp_token", Json::Str(token))?;
        }
        Ok(())
    }

    /// Port of `start(port=None)`.
    pub fn start(&mut self, port: Option<u16>) -> bool {
        if self.running {
            return true;
        }
        let _ = self.ensure_secrets();
        let Some(token) = self.token() else {
            self.error = Some(tr("AI 연결 설정을 저장하지 못했습니다."));
            self.note("server", "could not start");
            return false;
        };
        // Before binding: copies a crash left behind are gone even if it fails.
        self.elsewhere = !self.bridge.claim();
        if self.elsewhere {
            self.error = Some(tr("이 컴퓨터에서 실행 중인 다른 Grainy가 AI 연결을 쓰고 있습니다."));
            self.note("server", "on in another Grainy");
            return false;
        }
        // Also before binding: when the port cannot be used, the installed bridges still answer how to fix it.
        let bridges = self.write_bridges();
        let first = port.is_none() && self.port().is_none();
        let chosen = port.or(self.port()).unwrap_or(DEFAULT_PORT);
        let gui = Arc::new(GuiQueue::new({
            let wake = self.wake.clone();
            Box::new(move || wake())
        }));
        let tools: Arc<dyn grainy_mcp::Tools + Send + Sync> =
            Arc::new(GrainyTools::new(self.service.clone(), Some(gui.clone() as Arc<dyn Gui>)));
        let endpoint = |token: &str| Endpoint {
            protocol: Protocol::new(&self.version),
            tools: tools.clone(),
            service: self.service.clone(),
            token: token.to_owned(),
        };
        let mut bound = self.binder.bind(chosen, endpoint(&token));
        if bound.is_err() && first {
            for _ in 0..20 {
                let candidate = (self.ports)();
                let attempt = self.binder.bind(candidate, endpoint(&token));
                let ok = attempt.is_ok();
                bound = attempt;
                if ok {
                    break;
                }
            }
        }
        let server = match bound {
            Ok(server) => server,
            Err(failure) => {
                gui.close();
                self.bridge.release();
                self.error =
                    Some(tr_with("포트 {0}을(를) 쓸 수 없습니다: {1}", &[&chosen.to_string(), &strerror(&failure)]));
                self.note("server", "could not start");
                return false;
            }
        };
        self.server = Some(server);
        self.gui = Some(gui);
        self.running = true;
        self.error = None;
        if first {
            let bound_port = self.bound_port().unwrap_or(chosen);
            if self.save("mcp_port", Json::Int(i128::from(bound_port))).is_err() {
                self.note("server", "port not saved");
            }
        }
        if bridges {
            self.write_connection();
        }
        // The service's own state: running, a new generation (earlier jobs' results are not written), 'started'.
        self.service.start();
        true
    }

    /// Port of `start_if_enabled()`: at start-up, quiet; a failure is the status in Settings › AI Connection.
    pub fn start_if_enabled(&mut self) {
        if !self.enabled() {
            self.bridge.remove_stale();
            return;
        }
        self.start(None);
    }

    /// Port of `stop()`: idempotent. A running job is cancelled, waiting calls get Busy, `endpoint` and
    /// `auth-header` go (only when this process owns them) before the port is released.
    pub fn stop(&mut self) {
        let was = self.running || self.server.is_some();
        self.running = false;
        if let Some(gui) = self.gui.take() {
            gui.close();
        }
        if self.bridge.owned() {
            self.bridge.remove_files();
        }
        if let Some(server) = self.server.take() {
            server.stop();
        }
        self.bridge.release();
        if was {
            // running off, a new generation, the running job cancelled, 'stopped'.
            self.service.stop();
        } else {
            self.service.restart();
        }
    }

    /// Port of `set_enabled(on)`: the person's switch, remembered.
    pub fn set_enabled(&mut self, on: bool) -> bool {
        match self.save("mcp_enabled", Json::Bool(on)) {
            Err(_) => {
                self.error = Some(tr("AI 연결 설정을 저장하지 못했습니다."));
                if on {
                    return false;
                }
            }
            // An earlier failure no longer applies.
            Ok(()) => self.error = None,
        }
        if !on {
            self.stop();
            return true;
        }
        self.start(None)
    }

    /// Port of `regenerate_token()`: a new key (a failed save goes to the caller).
    pub fn regenerate_token(&mut self) -> std::io::Result<()> {
        let token = (self.tokens)();
        self.save("mcp_token", Json::Str(token))?;
        self.note("key", "new key");
        if self.running && self.write_bridges() {
            self.write_connection();
        }
        Ok(())
    }

    /// Port of `change_port()`: another random port, remembered.
    pub fn change_port(&mut self) -> bool {
        let old = self.bound_port().or(self.port());
        self.stop();
        for _ in 0..20 {
            let candidate = (self.ports)();
            if Some(candidate) == old {
                continue;
            }
            if self.start(Some(candidate)) {
                if self.save("mcp_port", Json::Int(i128::from(candidate))).is_err() {
                    self.error = Some(tr("AI 연결 설정을 저장하지 못했습니다."));
                }
                self.port_changed = true;
                return true;
            }
        }
        self.start(None)
    }

    /// Port of `write_bridges()`: the bridges and their answers while Grainy cannot be reached, rewritten when
    /// they differ (an update brings its new versions).
    pub fn write_bridges(&self) -> bool {
        let folder = self.bridge.folder();
        let result = (|| -> std::io::Result<()> {
            bridge::private_folder(folder)?;
            for (name, data) in BRIDGES {
                let target = folder.join(name);
                if std::fs::read(&target).ok().as_deref() != Some(data) {
                    bridge::write_private(&target, data)?;
                }
            }
            for (name, text) in Protocol::new(&self.version).offline_templates() {
                let target = folder.join(&name);
                if std::fs::read_to_string(&target).ok().as_deref() != Some(text.as_str()) {
                    bridge::write_private(&target, text.as_bytes())?;
                }
            }
            Ok(())
        })();
        if result.is_err() {
            self.note("files", "could not write the bridge files");
        }
        result.is_ok()
    }

    /// Port of `write_connection()`: while running and owning them, where and how the bridges reach Grainy.
    pub fn write_connection(&self) -> bool {
        let (Some(port), Some(token)) = (self.bound_port(), self.token()) else { return false };
        if !(self.running && self.bridge.owned()) {
            return false;
        }
        let written = self.bridge.write_connection(port, &token);
        if !written {
            self.note("files", "could not write the bridge files");
        }
        written
    }

    /// `owner is not None`.
    pub fn owned(&self) -> bool {
        self.bridge.owned()
    }

    /// The server the AI programs start (`install_commands(...)['claude_desktop_json']['mcpServers']['grainy']`).
    pub fn server_entry(&self, sys: &dyn System) -> Value {
        let windows = sys.os() == super::system::Os::Windows;
        let standard = standard_folder(sys, windows);
        install_commands(windows, Some(self.folder()), standard.as_deref())["claude_desktop_json"]["mcpServers"]
            ["grainy"]
            .clone()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if self.running || self.server.is_some() {
            self.stop();
        }
    }
}
