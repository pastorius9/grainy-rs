//! The Ask AI window's behaviour without its widgets (port of luma/ai_dialog.py `AiDialog`): the privacy notice,
//! the program list and its sign-in states, install and sign-in requests, sending a request to the chosen program
//! with what the person sees in Grainy, the transcript and the conversation. `dialogs/ask_ai.rs` draws it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use grainy_platform::json::Value as Json;
use grainy_platform::Preferences;
use serde_json::{json, Value};

use super::auth::{AuthController, AuthNews, AuthStatus};
use super::connection::Connection;
use super::install::{InstallController, Installer, NODE_WINDOWS};
use super::programs::{py_dumps, Event, Profile, Request, PROFILES};
use super::runner::{AiRunner, RunnerMessage};
use super::system::{Os, System};
use crate::text::{tr, tr_with};

/// `NOTICE`: what sending does, shown until the person acknowledges it.
pub const NOTICE: &str = "이 창에서 보내기를 누르면 선택한 AI 프로그램이 실행됩니다. AI는 사진 목록·파일 이름과 경로·사진 정보(GPS 제외)·보정값·미리보기와 밀착 이미지를 읽고, 읽은 내용을 자신의 AI 서비스로 보냅니다. 보정·분류 변경, 가져오기·내보내기, 스냅샷·가상 사본·밀착 이미지 저장도 할 수 있습니다. 프로그램의 기존 계정과 구독·사용 한도가 적용됩니다.";

/// What Ask AI needs of the AI connection (the oracle's `service` as `AiDialog` uses it).
pub trait Link {
    /// The settings file.
    fn preferences(&self) -> &Preferences;
    /// `preferences.save(key, value)`.
    fn save(&mut self, key: &str, value: Json) -> std::io::Result<()>;
    /// `service.enabled`.
    fn enabled(&self) -> bool;
    /// `service.running`.
    fn running(&self) -> bool;
    /// `service.error`.
    fn error(&self) -> Option<String>;
    /// `service.start()`.
    fn start(&mut self) -> bool;
    /// `service.set_enabled(on)`.
    fn set_enabled(&mut self, on: bool) -> bool;
    /// `service.write_bridges()`.
    fn write_bridges(&mut self) -> bool;
    /// `service.write_connection()`.
    fn write_connection(&mut self) -> bool;
    /// `service.folder`.
    fn folder(&self) -> PathBuf;
    /// `service.token`.
    fn token(&self) -> Option<String>;
}

impl Link for Connection {
    fn preferences(&self) -> &Preferences {
        Connection::preferences(self)
    }
    fn save(&mut self, key: &str, value: Json) -> std::io::Result<()> {
        Connection::save(self, key, value)
    }
    fn enabled(&self) -> bool {
        Connection::enabled(self)
    }
    fn running(&self) -> bool {
        self.running
    }
    fn error(&self) -> Option<String> {
        self.error.clone()
    }
    fn start(&mut self) -> bool {
        Connection::start(self, None)
    }
    fn set_enabled(&mut self, on: bool) -> bool {
        Connection::set_enabled(self, on)
    }
    fn write_bridges(&mut self) -> bool {
        Connection::write_bridges(self)
    }
    fn write_connection(&mut self) -> bool {
        Connection::write_connection(self)
    }
    fn folder(&self) -> PathBuf {
        Connection::folder(self).to_path_buf()
    }
    fn token(&self) -> Option<String> {
        Connection::token(self)
    }
}

/// What the person sees in Grainy now (`AiDialog.grainy_context`'s reads of the window).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Seen {
    /// `w.folder_filter`, with the subfolder box and how many photos it shows.
    pub folder: Option<(String, bool, usize)>,
    /// `w.selected_ids()`.
    pub selected: Vec<i64>,
    /// The open photo's id and file name.
    pub current: Option<(i64, String)>,
}

/// Port of `AiDialog.grainy_context`: English, for the AI.
pub fn grainy_context(seen: &Seen) -> String {
    let mut parts = Vec::new();
    let quoted = |s: &str| py_dumps(&json!(s), None, false);
    if let Some((folder, recursive, shown)) = &seen.folder {
        let folder = quoted(folder);
        parts.push(format!(
            "The folder open in Grainy is {folder} (list_photos with where folder={folder}, recursive {recursive}); it shows {shown} photos."
        ));
    }
    if !seen.selected.is_empty() {
        let mut shown = seen.selected.iter().take(50).map(i64::to_string).collect::<Vec<_>>().join(", ");
        if seen.selected.len() > 50 {
            shown.push_str(" …");
        }
        parts.push(format!("{} photos are selected (scope 'selection'; ids {shown}).", seen.selected.len()));
    }
    if let Some((id, name)) = &seen.current {
        if *id != 0 {
            parts.push(format!("The photo open in the editor is id {id}, {} (scope 'current').", quoted(name)));
        }
    }
    if parts.is_empty() {
        return String::new();
    }
    format!(
        "What the person sees in Grainy right now (read \"this folder\", \"these photos\", \"this photo\" with it): {}",
        parts.join(" ")
    )
}

/// The undo line of both AI windows (`McpDialog.refresh_undo`): the text and whether the button works.
pub fn undo_line(entry: Option<&Value>, job_running: bool) -> (String, bool) {
    let Some(entry) = entry else { return (tr("되돌릴 AI 작업이 없습니다."), false) };
    let when = entry.get("created").and_then(Value::as_str).and_then(local_minutes).unwrap_or_default();
    let label = match entry.get("label") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => grainy_mcp::pyjson::py_str(other),
        None => String::new(),
    };
    let count = entry.get("count").and_then(Value::as_i64).unwrap_or(0);
    (tr_with("{0} · {1}장 · {2}", &[&tr(&label), &thousands(count), &when]), !job_running)
}

/// `f'{n:,}'`.
pub(crate) fn thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// `datetime.fromisoformat(text).astimezone().strftime('%H:%M')` (a time without zone is local).
fn local_minutes(text: &str) -> Option<String> {
    use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
    let text = text.replacen(' ', "T", 1);
    if let Ok(aware) = DateTime::parse_from_rfc3339(&text) {
        return Some(aware.with_timezone(&Local).format("%H:%M").to_string());
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d"] {
        let parsed = NaiveDateTime::parse_from_str(&text, format)
            .ok()
            .or_else(|| chrono::NaiveDate::parse_from_str(&text, format).ok().and_then(|d| d.and_hms_opt(0, 0, 0)));
        if let Some(naive) = parsed {
            return Local.from_local_datetime(&naive).earliest().map(|t| t.format("%H:%M").to_string());
        }
    }
    None
}

/// Something the window must ask the person before an install (`confirm_install`, `node_prerequisite`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Question {
    /// 설치 확인: install `installer` for program `index`?
    Install { index: usize, installer: Installer },
    /// Node.js가 필요합니다: open the website, install Node.js with winget (Windows), or cancel.
    Node { index: usize },
}

/// The window's state as its widgets show it (`AiDialog.refresh`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub notice: bool,
    pub program: Option<usize>,
    pub send: bool,
    pub stop: bool,
    pub new: bool,
    pub enable_visible: bool,
    pub connection: String,
    pub hints: bool,
    pub login_visible: Vec<bool>,
    pub login_enabled: Vec<bool>,
    pub login_bold: Vec<bool>,
    pub install_visible: Vec<bool>,
    pub install_enabled: Vec<bool>,
    pub recheck_enabled: Vec<bool>,
    pub undo_label: String,
    pub undo_enabled: bool,
    pub program_enabled: bool,
}

/// A new conversation's run folder name (`uuid4().hex`).
fn run_key() -> String {
    super::connection::token_hex(16)
}

/// Port of `AiDialog` without widgets.
pub struct AskAi {
    sys: Arc<dyn System>,
    /// The programs, in the list's order.
    pub profiles: Vec<Profile>,
    /// What `detect` found for each program.
    pub executables: Vec<Option<Vec<String>>>,
    /// The run of the program.
    pub runner: AiRunner,
    /// Status checks and sign-in windows.
    pub auth: AuthController,
    /// Install windows.
    pub installs: InstallController,
    /// The status line of each program.
    pub auth_labels: Vec<String>,
    /// Programs whose last run asked to sign in again (their sign-in button is bold).
    pub auth_needed: BTreeSet<String>,
    session_id: String,
    session_choice: Option<(String, String)>,
    history: Vec<Value>,
    turn_text: String,
    /// A request is running.
    pub in_flight: bool,
    run_key: String,
    workspaces: BTreeSet<PathBuf>,
    /// The privacy notice shows.
    pub notice: bool,
    /// The chosen program.
    pub program: Option<usize>,
    /// 모델 (선택).
    pub model: String,
    /// The request being typed.
    pub input: String,
    /// The conversation (`QPlainTextEdit`).
    pub transcript: String,
    /// The status line.
    pub status: String,
    /// The sign-in / install help line, when shown.
    pub login_help: Option<String>,
    /// The undo message, when shown.
    pub undo_message: Option<String>,
    /// The programs whose status was asked for, in order (what `check` received).
    pub checks: Vec<String>,
    /// The last request started.
    pub last_request: Option<Request>,
}

impl AskAi {
    /// Port of `AiDialog.__init__`: the programs are looked for, the notice shows until acknowledged, the program
    /// chosen last (else the first one found) is selected.
    pub fn new(link: &dyn Link, sys: Arc<dyn System>, profiles: Vec<Profile>) -> Self {
        let executables: Vec<Option<Vec<String>>> = profiles.iter().map(|p| (p.detect)(sys.as_ref())).collect();
        let values = &link.preferences().values;
        let notice = !matches!(values.get("ai_notice_seen"), Some(v) if json_truthy(v));
        let saved = values.get("ai_program").and_then(Json::as_str).map(str::to_owned);
        let mut program = saved.and_then(|key| profiles.iter().position(|p| p.key == key));
        if program.is_none() {
            program = executables.iter().position(Option::is_some).or(if profiles.is_empty() { None } else { Some(0) });
        }
        let auth_labels = profiles
            .iter()
            .zip(&executables)
            .map(|(p, e)| {
                if e.is_some() {
                    tr_with("{0}: 확인할 수 없음", &[&tr(p.name)])
                } else {
                    tr_with("{0}: 설치되지 않음", &[&tr(p.name)])
                }
            })
            .collect();
        Self {
            runner: AiRunner::new(sys.clone()),
            auth: AuthController::new(sys.clone()),
            installs: InstallController::default(),
            sys,
            profiles,
            executables,
            auth_labels,
            auth_needed: BTreeSet::new(),
            session_id: String::new(),
            session_choice: None,
            history: Vec::new(),
            turn_text: String::new(),
            in_flight: false,
            run_key: run_key(),
            workspaces: BTreeSet::new(),
            notice,
            program,
            model: String::new(),
            input: String::new(),
            transcript: String::new(),
            status: tr("명령을 입력해 주세요."),
            login_help: None,
            undo_message: None,
            checks: Vec::new(),
            last_request: None,
        }
    }

    /// The standard program list.
    pub fn standard(link: &dyn Link, sys: Arc<dyn System>) -> Self {
        Self::new(link, sys, PROFILES.to_vec())
    }

    /// The system it works through.
    pub fn system(&self) -> &Arc<dyn System> {
        &self.sys
    }

    fn key(&self) -> Option<&'static str> {
        self.program.and_then(|i| self.profiles.get(i)).map(|p| p.key)
    }

    fn index_of(&self, key: &str) -> Option<usize> {
        self.profiles.iter().position(|p| p.key == key)
    }

    /// `QPlainTextEdit.appendPlainText`: a new paragraph.
    fn append(&mut self, text: &str) {
        if !self.transcript.is_empty() {
            self.transcript.push('\n');
        }
        self.transcript.push_str(text);
    }

    /// Port of `detect_program(index)`.
    pub fn detect_program(&mut self, index: usize) {
        let Some(profile) = self.profiles.get(index).copied() else { return };
        self.executables[index] = (profile.detect)(self.sys.as_ref());
        if self.executables[index].is_some() {
            self.check_auth(index);
        } else {
            self.auth_labels[index] = tr_with("{0}: 설치되지 않음", &[&tr(profile.name)]);
        }
    }

    /// Port of `check_auth(index)`.
    pub fn check_auth(&mut self, index: usize) {
        let Some(profile) = self.profiles.get(index).copied() else { return };
        let Some(executable) = self.executables[index].clone() else { return };
        self.auth_labels[index] = tr_with("{0}: 확인 중…", &[&tr(profile.name)]);
        self.checks.push(profile.key.into());
        self.auth.check(&profile, Some(executable));
    }

    /// Port of `auth_status_ready(key, status)`.
    pub fn auth_status_ready(&mut self, key: &str, status: &AuthStatus) {
        let Some(index) = self.index_of(key) else { return };
        if self.executables[index].is_none() {
            return;
        }
        let mut text = match status.state.as_str() {
            "signed_in" => tr("로그인됨"),
            "signed_out" => tr("로그인되지 않음"),
            "timeout" => tr("응답 없음"),
            _ => tr("확인할 수 없음"),
        };
        if ["Claude", "ChatGPT", "Google", "API key", "Cloud"].contains(&status.kind.as_str()) {
            text = tr_with("{0} ({1})", &[&text, &tr(&status.kind)]);
        }
        self.auth_labels[index] = tr_with("{0}: {1}", &[&tr(self.profiles[index].name), &text]);
    }

    /// Port of `sign_in(index)`.
    pub fn sign_in(&mut self, index: usize, settings_folder: &Path) {
        if self.in_flight || self.executables.get(index).is_none_or(Option::is_none) {
            return;
        }
        let profile = self.profiles[index];
        let executable = self.executables[index].clone();
        self.login_help = Some(match self.auth.login(&profile, executable.as_deref(), settings_folder) {
            Err(_) => tr("로그인 창을 열지 못했습니다. 프로그램 설치를 확인해 주세요."),
            Ok(()) if profile.key == "gemini" => tr(
                "Gemini 창에서 \"Login with Google\"을 선택하세요. 이미 대화 화면이면 /auth로 로그인을 바꿀 수 있습니다. 완료 후 콘솔 창을 닫고 다시 확인하세요.",
            ),
            Ok(()) => tr("열린 프로그램 창에서 로그인을 마친 뒤 창을 닫으세요. 상태는 자동으로 다시 확인합니다."),
        });
    }

    /// Port of `sign_in_closed(key)`.
    pub fn sign_in_closed(&mut self, key: &str) {
        if let Some(index) = self.index_of(key) {
            self.check_auth(index);
        }
    }

    /// Port of the first half of `install_program(index)`: what to ask before an install, if one can start.
    pub fn install_program(&mut self, index: usize) -> Option<Question> {
        if self.in_flight
            || !self.installs.sessions.is_empty()
            || self.executables.get(index).is_none_or(Option::is_some)
        {
            return None;
        }
        let installer = *self.profiles[index].installer(self.sys.os())?;
        if installer.needs_node && !super::discover::node_ready(self.sys.as_ref()) {
            return Some(Question::Node { index });
        }
        Some(Question::Install { index, installer })
    }

    /// The Node.js box answered (`node_prerequisite`): the website was asked for (`website`), or the winget install
    /// (Windows): the next question, if any.
    pub fn node_answer(&mut self, index: usize, website: bool, winget: bool) -> Option<Question> {
        if website {
            if !self.sys.open_url(super::install::NODE_URL) {
                self.login_help = Some(tr_with(
                    "브라우저를 열지 못했습니다. 공식 주소를 직접 열어 주세요: {0}",
                    &[super::install::NODE_URL],
                ));
            }
            return None;
        }
        (winget && self.sys.os() == Os::Windows).then_some(Question::Install { index, installer: NODE_WINDOWS })
    }

    /// The install was confirmed: the installer starts in its own console (`installs.start`).
    pub fn install_confirmed(&mut self, index: usize, installer: &Installer, settings_folder: &Path) {
        let key = self.profiles[index].key;
        let sys = self.sys.clone();
        self.login_help = Some(match self.installs.start(key, installer, settings_folder, sys.as_ref()) {
            Err(_) => tr("설치 창을 열지 못했습니다. 공식 안내에서 설치 방법을 확인해 주세요."),
            Ok(_) => tr(
                "열린 설치 창에서 결과를 확인한 뒤 아무 키나 누르세요. 프로그램을 다시 찾습니다. 필요하면 다시 확인을 누르세요.",
            ),
        });
    }

    /// Port of `install_closed(key)`.
    pub fn install_closed(&mut self, key: &str) {
        let Some(index) = self.index_of(key) else { return };
        self.detect_program(index);
        self.login_help = Some(if self.executables[index].is_some() {
            tr("프로그램을 찾았습니다. 로그인 상태를 확인하고 로그인…을 누르세요.")
        } else {
            tr("프로그램을 아직 찾지 못했습니다. 설치 창의 결과를 확인하세요. Node.js만 설치했다면 설치…를 다시 눌러 AI 프로그램을 설치하세요.")
        });
    }

    /// Port of `acknowledge()`.
    pub fn acknowledge(&mut self, link: &mut dyn Link) {
        if link.save("ai_notice_seen", Json::Bool(true)).is_err() {
            self.status = tr("AI 창 설정을 저장하지 못했습니다.");
            return;
        }
        self.notice = false;
    }

    /// Port of `choice_changed()` (another program or model).
    pub fn choice_changed(&mut self, link: &mut dyn Link) {
        if self.runner.running() {
            return;
        }
        let now = (self.key().unwrap_or("").to_owned(), self.model.trim().to_owned());
        if self.session_choice.as_ref().is_some_and(|c| *c != now) {
            self.new_conversation();
        }
        if let Some(key) = self.key() {
            if link.save("ai_program", Json::from(key)).is_err() {
                self.status = tr("AI 창 설정을 저장하지 못했습니다.");
            }
        }
        if let Some(index) = self.program {
            self.check_auth(index);
        }
    }

    /// Port of `enable_connection()`.
    pub fn enable_connection(&mut self, link: &mut dyn Link) {
        link.set_enabled(true);
    }

    /// Port of `refresh()`.
    pub fn view(&self, link: &dyn Link, undo: (String, bool)) -> View {
        let running = self.in_flight;
        let key = self.key();
        let send = !running
            && self.program.is_some()
            && link.enabled()
            && self.program.is_some_and(|i| self.executables[i].is_some())
            && key.is_none_or(|k| !self.installs.sessions.contains_key(k))
            && matches!(link.preferences().values.get("ai_notice_seen"), Some(v) if json_truthy(v));
        let connection = if !link.enabled() {
            tr("AI 연결이 꺼져 있습니다. 켠 뒤 보내 주세요.")
        } else {
            link.error().unwrap_or_else(|| tr("Grainy 도구로 사진을 읽고 작업합니다."))
        };
        let (undo_label, mut undo_enabled) = undo;
        if running {
            undo_enabled = false;
        }
        let installed: Vec<bool> = self.executables.iter().map(Option::is_some).collect();
        let os = self.sys.os();
        View {
            notice: self.notice,
            program: self.program,
            send,
            stop: running,
            new: !running,
            enable_visible: !link.enabled(),
            connection,
            hints: !installed.iter().all(|i| *i),
            login_visible: installed.clone(),
            login_enabled: self
                .profiles
                .iter()
                .map(|p| {
                    !running && !self.auth.logins.contains_key(p.key) && !self.installs.sessions.contains_key(p.key)
                })
                .collect(),
            login_bold: self.profiles.iter().map(|p| self.auth_needed.contains(p.key)).collect(),
            install_visible: installed.iter().map(|i| !i).collect(),
            install_enabled: self
                .profiles
                .iter()
                .map(|p| !running && self.installs.sessions.is_empty() && p.installer(os).is_some())
                .collect(),
            recheck_enabled: self.profiles.iter().map(|_| !running).collect(),
            undo_label,
            undo_enabled,
            program_enabled: !running,
        }
    }

    /// Port of `send()`: the request goes to the chosen program with what the person sees (`seen`).
    pub fn send(&mut self, link: &mut dyn Link, seen: &Seen, undo: (String, bool)) {
        if !self.view(link, undo).send {
            return;
        }
        let prompt = self.input.trim().to_owned();
        if prompt.is_empty() {
            return;
        }
        if prompt.encode_utf16().count() * 2 > 16000 {
            self.status = tr("요청이 너무 깁니다. 8,000자 이내로 줄여 주세요.");
            return;
        }
        if !link.running() && !link.start() {
            return;
        }
        // A running HTTP server can have failed to write its bridge on startup.
        if !link.write_bridges() || !link.write_connection() {
            self.status = tr("AI 중계 파일을 준비하지 못했습니다. AI 연결 설정을 확인해 주세요.");
            return;
        }
        let Some(index) = self.program else { return };
        let profile = self.profiles[index];
        let mut sent = prompt.clone();
        if !self.history.is_empty() && !profile.persistent {
            let mut recent = self.history.clone();
            let mut context = py_dumps(&Value::Array(recent.clone()), None, false);
            while context.encode_utf16().count() * 2 > 12000 && !recent.is_empty() {
                recent = recent.split_off(2.min(recent.len()));
                context = py_dumps(&Value::Array(recent.clone()), None, false);
            }
            sent = format!(
                "Previous conversation, for context only; do not repeat its completed actions:\n{context}\n\nNew request:\n{prompt}"
            );
            self.append(&tr("기록 저장을 끈 새 실행에 이전 대화 내용을 전달합니다."));
        }
        let windows = self.sys.os() == Os::Windows;
        let standard = super::connection::standard_folder(self.sys.as_ref(), windows);
        let server = super::connection::install_commands(windows, Some(&link.folder()), standard.as_deref())
            ["claude_desktop_json"]["mcpServers"]["grainy"]
            .clone();
        let settings_folder = link.preferences().path.parent().map(Path::to_path_buf).unwrap_or_default();
        let workspace = settings_folder.join("ai-workspace").join(profile.key).join(&self.run_key);
        self.workspaces.insert(workspace.clone());
        let request = Request {
            executable: self.executables[index].clone().unwrap_or_default(),
            workspace,
            server,
            prompt: sent,
            language: grainy_i18n::language().code().to_owned(),
            model: self.model.trim().to_owned(),
            session_id: self.session_id.clone(),
            context: grainy_context(seen),
        };
        let secrets: Vec<String> = link.token().into_iter().collect();
        let started = link
            .save("ai_program", Json::from(profile.key))
            .map_err(|e| e.to_string())
            .and_then(|()| self.runner.start(&profile, &request, &secrets).map_err(|e| e.0));
        if started.is_err() {
            self.status = tr("AI 프로그램을 시작하지 못했습니다. 설치와 계정 로그인을 확인해 주세요.");
            return;
        }
        self.session_choice = Some((profile.key.into(), request.model.clone()));
        self.last_request = Some(request);
        self.in_flight = true;
        self.history.push(json!({"role": "user", "content": prompt.chars().take(8000).collect::<String>()}));
        self.turn_text.clear();
        self.append(&format!("{}\n", tr_with("나: {0}", &[&prompt])));
        self.input.clear();
        self.status = tr("AI가 작업 중입니다…");
    }

    /// Port of `on_event(event)`.
    pub fn on_event(&mut self, event: &Event) {
        match event.kind.as_str() {
            "session" => self.session_id = event.text.clone(),
            "text" => {
                let joined = format!("{}{}", self.turn_text, event.text);
                let skip = joined.chars().count().saturating_sub(8000);
                self.turn_text = joined.chars().skip(skip).collect();
                self.transcript.push_str(&event.text);
            }
            "tool" => {
                let text = match event.count {
                    None => event.text.clone(),
                    Some(n) => tr_with("{0} · 대상 {1}장", &[&event.text, &n.to_string()]),
                };
                self.append(&format!("\n{text}\n"));
            }
            "auth_error" => {
                if let Some(index) = self.program {
                    let profile = self.profiles[index];
                    self.auth_needed.insert(profile.key.into());
                    self.append(&tr_with(
                        "로그인이 필요합니다. '로그인…'을 눌러 {0}에 다시 로그인하세요.",
                        &[&tr(profile.name)],
                    ));
                }
            }
            "error" => self.append(&tr("AI 프로그램 오류가 발생했습니다. 다시 시도해 주세요.")),
            "launch_error" | "exit" => self.append(&tr(
                "AI 프로그램이 종료되었습니다. 터미널에서 해당 프로그램의 설치·로그인·사용 한도를 확인해 주세요.",
            )),
            _ => {}
        }
    }

    /// Port of `on_finished(result)`.
    pub fn on_finished(&mut self, result: &str) {
        self.in_flight = false;
        self.history.push(json!({"role": "assistant", "content": self.turn_text, "status": result}));
        let skip = self.history.len().saturating_sub(6);
        self.history.drain(..skip);
        if result == "done" {
            if let Some(key) = self.key() {
                self.auth_needed.remove(key);
            }
        }
        let text = match result {
            "done" => tr("완료"),
            "stopped" => tr("중지됨 · 이미 적용한 작업은 유지됩니다."),
            "timeout" => tr("시간 초과 · 프로그램을 중지했습니다."),
            "auth_required" => tr("로그인이 필요합니다."),
            "failed" => tr("실패 · 프로그램의 로그인과 사용 한도를 확인해 주세요."),
            _ => tr("실패"),
        };
        self.append(&format!("\n{text}\n"));
        self.status = text;
    }

    /// Port of `stop()`.
    pub fn stop(&mut self) {
        self.runner.stop();
        self.status = tr("중지하는 중…");
    }

    /// Port of `new_conversation()`.
    pub fn new_conversation(&mut self) {
        if self.runner.running() {
            return;
        }
        self.session_id.clear();
        self.runner.clear_session();
        self.session_choice = None;
        self.history.clear();
        self.turn_text.clear();
        self.clean_workspaces();
        self.run_key = run_key();
        self.transcript.clear();
        self.status = tr("새 대화를 시작합니다.");
    }

    /// Port of `clean_workspaces()`: only this window's run folders, after their programs stopped.
    pub fn clean_workspaces(&mut self) {
        if self.runner.running() {
            return;
        }
        for path in self.workspaces.clone() {
            let Some(root) = path.parent().and_then(Path::parent) else { continue };
            let ours = root.file_name().is_some_and(|n| n == "ai-workspace")
                && path.file_name().is_some_and(|n| n.to_string_lossy() == self.run_key);
            if !ours {
                continue;
            }
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => continue,
            }
            self.workspaces.remove(&path);
        }
    }

    /// Port of `showEvent`: the programs are looked for again (and their sign-in checked).
    pub fn shown(&mut self) {
        for index in 0..self.profiles.len() {
            self.detect_program(index);
        }
    }

    /// The timers and signals of the window: the run's events and end, finished status checks, closed sign-in and
    /// install windows.
    pub fn tick(&mut self) {
        for message in self.runner.poll() {
            match message {
                RunnerMessage::Event(event) => self.on_event(&event),
                RunnerMessage::Finished(result) => self.on_finished(&result),
            }
        }
        for news in self.auth.poll() {
            match news {
                AuthNews::Status(key, status) => self.auth_status_ready(&key, &status),
                AuthNews::LoginClosed(key) => self.sign_in_closed(&key),
            }
        }
        for key in self.installs.poll() {
            self.install_closed(&key);
        }
    }

    /// Port of `shutdown()` / `closeEvent`: the run stops and its folders go.
    pub fn shutdown(&mut self) {
        self.runner.shutdown();
        self.clean_workspaces();
    }

    /// Whether work is in progress (a run, a status check).
    pub fn busy(&self) -> bool {
        self.in_flight || self.auth.checking()
    }
}

/// Python's truth value of a settings value.
fn json_truthy(value: &Json) -> bool {
    match value {
        Json::Null => false,
        Json::Bool(b) => *b,
        Json::Int(i) => *i != 0,
        Json::Float(f) => *f != 0.0,
        Json::Str(s) => !s.is_empty(),
        Json::List(l) => !l.is_empty(),
        Json::Object(o) => !o.is_empty(),
    }
}
