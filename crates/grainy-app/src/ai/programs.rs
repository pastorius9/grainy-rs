//! The AI programs Ask AI can run (port of luma/ai_programs.py): the request with its instruction, the argument
//! lists and per-run files of Claude Code, Codex and Gemini CLI, the decoding of their JSON event streams, the
//! profiles table and the redaction of secrets. Files go to the run's workspace only (in Grainy's settings folder).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::discover;
use super::install::{Installer, INSTALLERS};
use super::system::{Os, System};

/// Python's `json.dumps(value, ensure_ascii=…, indent=…)` (default separators).
pub fn py_dumps(value: &Value, indent: Option<usize>, ensure_ascii: bool) -> String {
    grainy_platform::json::Value::from(value).dump(indent, ensure_ascii)
}

/// `write_json(path, value)`: UTF-8, `indent=2`, `ensure_ascii=False`; on POSIX readable by this user only.
pub fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, py_dumps(value, Some(2), false))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// `urllib.parse.quote_from_bytes(path, safe='/')`.
fn quote(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"_.-~/".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `Path.as_uri()` of an absolute path.
pub fn as_uri(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let bytes = text.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' {
        format!("file:///{}{}", &text[..2], quote(&text[2..]))
    } else if text.starts_with("//") {
        format!("file:{}", quote(&text))
    } else {
        format!("file://{}", quote(&text))
    }
}

/// Port of `Request`: one turn of Ask AI.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// The program (`find_program` / `codex_program`).
    pub executable: Vec<String>,
    /// This window's run folder (`ai-workspace/<program>/<run key>`).
    pub workspace: PathBuf,
    /// Grainy's MCP server as the program starts it (`{'command', 'args'}`).
    pub server: Value,
    /// What is sent (the person's text, with the earlier conversation for programs without sessions).
    pub prompt: String,
    /// The interface language (`ko` / `en`).
    pub language: String,
    /// The chosen model, or empty for the program's default.
    pub model: String,
    /// The program's session to continue, or empty.
    pub session_id: String,
    /// What the person sees in Grainy now (`AiDialog.grainy_context`).
    pub context: String,
}

impl Request {
    /// Port of the `instruction` property: the system prompt.
    pub fn instruction(&self) -> String {
        let language = if self.language == "ko" { "Korean" } else { "English" };
        let text = format!(
            "You are working inside Grainy, a photo editor, for the person using it. Use only the grainy MCP tools to \
             inspect and change the photo library. Answer in {language}. Check tool results before reporting success. \
             Do not execute commands, edit files, browse the web, or use other services. For a job, use get_job \
             until it finishes. Never invent photo ids, names or results: if a tool is missing or fails, say so."
        );
        if self.context.is_empty() {
            text
        } else {
            format!("{text} {}", self.context)
        }
    }
}

/// Why a program's arguments could not be built (Python's exception class and text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// `ValueError`.
    Value(String),
    /// `OSError` (a file could not be written).
    Io(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Value(text) => write!(f, "ValueError: {text}"),
            BuildError::Io(text) => write!(f, "OSError: {text}"),
        }
    }
}

impl From<std::io::Error> for BuildError {
    fn from(error: std::io::Error) -> Self {
        BuildError::Io(error.to_string())
    }
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn validate_options(r: &Request) -> Result<(), BuildError> {
    if r.model.starts_with('-') || r.session_id.starts_with('-') {
        return Err(BuildError::Value(crate::text::tr("모델 이름과 세션 ID는 하이픈으로 시작할 수 없습니다.")));
    }
    Ok(())
}

/// Port of `claude_command`.
pub fn claude_command(r: &Request, _sys: &dyn System) -> Result<Vec<String>, BuildError> {
    validate_options(r)?;
    let config = r.workspace.join("claude-mcp.json");
    write_json(&config, &json!({"mcpServers": {"grainy": r.server}}))?;
    let mut args = r.executable.clone();
    args.extend(
        [
            "-p",
            &r.prompt,
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--mcp-config",
            &path_text(&config),
            "--strict-mcp-config",
            "--tools",
            "",
            "--allowedTools",
            "mcp__grainy",
            "--permission-mode",
            "dontAsk",
            "--append-system-prompt",
            &r.instruction(),
            "--disable-slash-commands",
            "--no-chrome",
            "--setting-sources",
            "",
            "--settings",
            "{\"disableAllHooks\":true,\"autoMemoryEnabled\":false}",
        ]
        .map(str::to_owned),
    );
    if !r.session_id.is_empty() {
        args.extend(["--resume".to_owned(), r.session_id.clone()]);
    }
    if !r.model.is_empty() {
        args.extend(["--model".to_owned(), r.model.clone()]);
    }
    if r.prompt.starts_with('-') {
        args.remove(r.executable.len() + 1);
        args.extend(["--".into(), r.prompt.clone()]);
    }
    Ok(args)
}

/// `CODEX_DISABLED`: Codex features switched off for a run (validated by the oracle against Codex 0.163).
pub const CODEX_DISABLED: [&str; 38] = [
    "shell_tool",
    "unified_exec",
    "shell_snapshot",
    "apply_patch_freeform",
    "apps",
    "connectors",
    "plugins",
    "remote_plugin",
    "hooks",
    "codex_hooks",
    "plugin_hooks",
    "multi_agent",
    "collab",
    "multi_agent_v2",
    "js_repl",
    "browser_use",
    "computer_use",
    "image_generation",
    "imagegenext",
    "view_image",
    "memory_tool",
    "memories",
    "skill_search",
    "request_permissions",
    "request_permissions_tool",
    "goals",
    "tool_search",
    "code_mode",
    "code_mode_only",
    "sleep_tool",
    "stable_environment_tools",
    "current_time_reminder",
    "token_budget",
    "deferred_executor",
    "send_message_to_user_async",
    "tool_suggest",
    "recommended_plugins",
    "skill_mcp_dependency_install",
];

/// Port of `codex_catalog`: Codex's normal model cache (read, never modified) with the tools that feature flags do
/// not control removed, written as this run's static catalogue.
pub fn codex_catalog(r: &Request, sys: &dyn System) -> Result<PathBuf, BuildError> {
    let home = sys.var("CODEX_HOME").filter(|h| !h.is_empty()).map_or_else(|| sys.home().join(".codex"), PathBuf::from);
    let cache = home.join("models_cache.json");
    let cached: Option<Value> =
        std::fs::read(&cache).ok().and_then(|b| String::from_utf8(b).ok()).and_then(|t| serde_json::from_str(&t).ok());
    let mut models: Vec<Value> = match &cached {
        Some(Value::Object(map)) => match map.get("models") {
            Some(Value::Array(list)) => list.clone(),
            None => Vec::new(),
            Some(_) => Vec::new(),
        },
        _ => Vec::new(),
    };
    if !models.iter().all(Value::is_object) {
        models = Vec::new();
    }
    if models.is_empty() && r.model.is_empty() {
        return Err(BuildError::Value("Codex needs its model cache or an explicit model".into()));
    }
    for model in &mut models {
        if let Value::Object(map) = model {
            for (key, value) in [
                ("apply_patch_tool_type", Value::Null),
                ("shell_type", json!("disabled")),
                ("experimental_supported_tools", json!([])),
                ("supports_search_tool", json!(false)),
                ("include_skills_usage_instructions", json!(false)),
                ("include_plugin_usage_instructions", json!(false)),
                ("include_apps_usage_instructions", json!(false)),
                ("tool_mode", Value::Null),
                ("multi_agent_version", Value::Null),
            ] {
                map.insert(key.into(), value);
            }
        }
    }
    let path = r.workspace.join("codex-models.json");
    write_json(&path, &json!({"models": models}))?;
    Ok(path)
}

/// Port of `codex_command`.
pub fn codex_command(r: &Request, sys: &dyn System) -> Result<Vec<String>, BuildError> {
    validate_options(r)?;
    let mut args = r.executable.clone();
    args.extend(
        [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--ignore-user-config",
            "--ignore-rules",
            "--ephemeral",
            "-C",
            &path_text(&r.workspace),
            "-s",
            "read-only",
        ]
        .map(str::to_owned),
    );
    let catalog = codex_catalog(r, sys)?;
    let mut config: Vec<(String, Value)> = vec![
        ("approval_policy".into(), json!("never")),
        ("web_search".into(), json!("disabled")),
        ("model_catalog_json".into(), json!(path_text(&catalog))),
        ("agents.enabled".into(), json!(false)),
        ("tools.experimental_request_user_input.enabled".into(), json!(false)),
        ("mcp_servers.grainy.command".into(), r.server["command"].clone()),
        ("mcp_servers.grainy.args".into(), r.server["args"].clone()),
        ("mcp_servers.grainy.default_tools_approval_mode".into(), json!("approve")),
        ("mcp_servers.grainy.startup_timeout_sec".into(), json!(30)),
        ("developer_instructions".into(), json!(r.instruction())),
        ("project_doc_max_bytes".into(), json!(0)),
        ("features.skip_host_skill_discovery".into(), json!(true)),
        ("tools.update_plan.enabled".into(), json!(false)),
        ("history.persistence".into(), json!("none")),
        ("log_dir".into(), json!(path_text(&r.workspace.join("codex-log")))),
        ("sqlite_home".into(), json!(path_text(&r.workspace.join("codex-state")))),
    ];
    config.extend(CODEX_DISABLED.iter().map(|name| (format!("features.{name}"), json!(false))));
    for (key, value) in config {
        args.push("-c".into());
        args.push(format!("{key}={}", py_dumps(&value, None, false)));
    }
    if !r.model.is_empty() {
        args.extend(["-m".to_owned(), r.model.clone()]);
    }
    if r.prompt.starts_with('-') {
        args.push("--".into());
    }
    args.push(r.prompt.clone());
    Ok(args)
}

/// Port of `gemini_command`: Gemini's settings for this run, and a small Node entry that keeps its conversation
/// storage in the run folder (it never opens the credentials).
pub fn gemini_command(r: &Request, _sys: &dyn System) -> Result<Vec<String>, BuildError> {
    validate_options(r)?;
    let mut server = r.server.as_object().cloned().unwrap_or_default();
    server.insert("trust".into(), json!(true));
    let settings = json!({
        "mcpServers": {"grainy": Value::Object(server)},
        "mcp": {"allowed": ["grainy"], "serverCommand": ""},
        "tools": {"core": [], "allowed": ["grainy"], "discoveryCommand": "", "callCommand": ""},
        "hooksConfig": {"enabled": false}, "context": {"fileName": []},
        "general": {"enableAutoUpdate": false}, "telemetry": {"enabled": false},
        "experimental": {"enableAgents": false},
        "agents": {"overrides": {"codebase_investigator": {"enabled": false}, "cli_help": {"enabled": false}}},
        "security": {"folderTrust": {"enabled": false}}, "admin": {"extensions": {"enabled": false}},
    });
    write_json(&r.workspace.join(".gemini").join("settings.json"), &settings)?;
    if r.executable.len() != 2 || !r.executable[1].ends_with(".js") {
        return Err(BuildError::Value("Gemini needs the supported npm installation".into()));
    }
    let entry = discover::resolve(Path::new(&r.executable[1]));
    let mut storage = entry.parent().and_then(Path::parent).map(Path::to_path_buf).unwrap_or_default();
    for part in ["node_modules", "@google", "gemini-cli-core", "dist", "src", "config", "storage.js"] {
        storage.push(part);
    }
    if !storage.is_file() {
        return Err(BuildError::Value("Unsupported Gemini storage layout".into()));
    }
    let wrapper = r.workspace.join("gemini-entry.mjs");
    let state = path_text(&r.workspace.join("gemini-state"));
    let dumps = |text: &str| py_dumps(&json!(text), None, true);
    let text = format!(
        "import {{Storage}} from {};\nStorage.getGlobalTempDir = () => {};\nStorage.prototype.initialize = async \
         function() {{ this.projectIdentifier = \"grainy\"; }};\nStorage.prototype.getHistoryDir = function() {{ return \
         {}; }};\nprocess.argv[1] = {};\nawait import({});\n",
        dumps(&as_uri(&storage)),
        dumps(&state),
        dumps(&format!("{state}/history")),
        dumps(&path_text(&entry)),
        dumps(&as_uri(&entry)),
    );
    std::fs::write(&wrapper, text)?;
    let mut args = vec![
        r.executable[0].clone(),
        path_text(&wrapper),
        "--allowed-mcp-server-names".into(),
        "grainy".into(),
        "--approval-mode".into(),
        "default".into(),
        "--extensions".into(),
        "none".into(),
        "--output-format".into(),
        "stream-json".into(),
        "-p".into(),
        format!("{}{}\n\n{}", if r.prompt.starts_with('-') { " " } else { "" }, r.prompt, r.instruction()),
    ];
    if !r.model.is_empty() {
        args.extend(["--model".to_owned(), r.model.clone()]);
    }
    if !r.session_id.is_empty() {
        args.extend(["--resume".to_owned(), r.session_id.clone()]);
    }
    Ok(args)
}

/// Port of `Event`: what a program's stream says, as the window shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    /// `session`, `text`, `tool`, `error`, `auth_error`, `launch_error`, `exit`.
    pub kind: String,
    /// The text (a session id, text, a tool name, an exit code).
    pub text: String,
    /// How many photos a tool call names, when it names them.
    pub count: Option<usize>,
}

impl Event {
    /// An event with text.
    pub fn new(kind: &str, text: impl Into<String>, count: Option<usize>) -> Self {
        Self { kind: kind.into(), text: text.into(), count }
    }
}

/// Python's `str()` of a JSON value read by `json.loads`.
fn py_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => grainy_mcp::pyjson::py_str(other),
    }
}

/// Port of `tool_event`: only names of the shipped tool list, never arguments or results.
pub fn tool_event(name: &Value, arguments: &Value) -> Event {
    let text = py_text(name);
    let text = text.strip_prefix("mcp__grainy__").unwrap_or(&text);
    let text = text.strip_prefix("grainy__").unwrap_or(text).to_owned();
    let known = grainy_mcp::tools::definitions().iter().any(|d| d["name"].as_str() == Some(text.as_str()));
    if !known {
        return Event::new("error", "Unexpected tool outside Grainy", None);
    }
    let empty = Map::new();
    let args = arguments.as_object().unwrap_or(&empty);
    let mut count = args.get("ids").and_then(Value::as_array).map(Vec::len);
    if let Some(Value::Array(groups)) = args.get("edits") {
        if groups.iter().all(|g| g.get("ids").is_some_and(Value::is_array)) {
            let mut seen = HashSet::new();
            for group in groups {
                for id in group["ids"].as_array().into_iter().flatten() {
                    // Python's isinstance(i, int): True and False count as 1 and 0.
                    let key = match id {
                        Value::Bool(b) => Some(i128::from(*b)),
                        Value::Number(n) if n.is_i64() || n.is_u64() => {
                            n.as_i64().map(i128::from).or_else(|| n.as_u64().map(i128::from))
                        }
                        _ => None,
                    };
                    if let Some(key) = key {
                        seen.insert(key);
                    }
                }
            }
            count = Some(seen.len());
        }
    }
    Event::new("tool", text, count)
}

/// A stream line Python could not decode (`ValueError`, `KeyError`, `AttributeError`…): the runner skips it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Undecodable;

type Decoded = Result<Vec<Event>, Undecodable>;

/// `data.get(key, {})` that must be a dict (a present non-dict is Python's `AttributeError` on the next `.get`).
fn sub<'a>(
    data: &'a Map<String, Value>,
    key: &str,
    empty: &'a Map<String, Value>,
) -> Result<&'a Map<String, Value>, Undecodable> {
    match data.get(key) {
        None => Ok(empty),
        Some(Value::Object(map)) => Ok(map),
        Some(_) => Err(Undecodable),
    }
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) | Some(Value::Bool(false)) => false,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64() != Some(0.0),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
        Some(Value::Bool(true)) => true,
    }
}

fn text_of(data: &Map<String, Value>, key: &str) -> String {
    data.get(key).map(py_text).unwrap_or_default()
}

/// Port of `EventParser`: the state of one run's stream, so streamed deltas and final messages do not repeat.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventParser {
    seen: HashSet<String>,
    /// Text arrived as deltas since the message started.
    pub partial: bool,
    /// The stream reported success.
    pub succeeded: bool,
    /// The stream reported a failure.
    pub failed: bool,
}

impl EventParser {
    /// A parser for a new run.
    pub fn new() -> Self {
        Self::default()
    }

    fn error(&mut self, data: &Value) -> Vec<Event> {
        self.failed = true;
        vec![Event::new(if super::auth::authentication_failed_value(data) { "auth_error" } else { "error" }, "", None)]
    }

    fn once(&mut self, key: Option<&Value>, event: Event) -> Vec<Event> {
        let key = key.map_or_else(|| "None".to_owned(), |k| k.to_string());
        if !self.seen.insert(key) {
            return Vec::new();
        }
        vec![event]
    }

    /// Port of `EventParser.claude`.
    pub fn claude(&mut self, data: &Value) -> Decoded {
        let empty = Map::new();
        let data = data.as_object().ok_or(Undecodable)?;
        let kind = data.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "system" && truthy(data.get("session_id")) {
            return Ok(vec![Event::new("session", text_of(data, "session_id"), None)]);
        }
        if kind == "error" || (kind == "assistant" && truthy(data.get("error"))) {
            return Ok(self.error(&Value::Object(data.clone())));
        }
        if kind == "stream_event" && sub(data, "event", &empty)?.get("type") == Some(&json!("error")) {
            return Ok(self.error(data.get("event").unwrap_or(&Value::Null)));
        }
        if kind == "stream_event" {
            let event = sub(data, "event", &empty)?;
            if event.get("type") == Some(&json!("message_start")) {
                self.partial = false;
            }
            let delta = sub(event, "delta", &empty)?;
            if delta.get("type") == Some(&json!("text_delta")) {
                self.partial = true;
                return Ok(vec![Event::new("text", text_of(delta, "text"), None)]);
            }
        }
        if kind == "assistant" {
            let mut out = Vec::new();
            let message = sub(data, "message", &empty)?;
            let parts = match message.get("content") {
                None => Vec::new(),
                Some(Value::Array(list)) => list.clone(),
                Some(_) => return Err(Undecodable),
            };
            for part in &parts {
                let part = part.as_object().ok_or(Undecodable)?;
                let ty = part.get("type");
                if ty == Some(&json!("text")) && !self.partial {
                    out.push(Event::new("text", text_of(part, "text"), None));
                } else if ty == Some(&json!("tool_use")) {
                    let event =
                        tool_event(part.get("name").unwrap_or(&Value::Null), part.get("input").unwrap_or(&Value::Null));
                    out.extend(self.once(part.get("id"), event));
                }
            }
            self.partial = false;
            return Ok(out);
        }
        if kind == "result" {
            self.failed = self.failed || truthy(data.get("is_error"));
            self.succeeded = !self.failed;
            return Ok(if self.failed { self.error(&Value::Object(data.clone())) } else { Vec::new() });
        }
        Ok(Vec::new())
    }

    /// Port of `EventParser.codex`.
    pub fn codex(&mut self, data: &Value) -> Decoded {
        let empty = Map::new();
        let data = data.as_object().ok_or(Undecodable)?;
        let kind = data.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "thread.started" {
            let id = data.get("thread_id").ok_or(Undecodable)?;
            return Ok(vec![Event::new("session", py_text(id), None)]);
        }
        let item = sub(data, "item", &empty)?;
        let item_type = item.get("type");
        if kind == "item.completed" && item_type == Some(&json!("agent_message")) {
            let event = Event::new("text", format!("{}\n", text_of(item, "text")), None);
            return Ok(self.once(item.get("id"), event));
        }
        if (kind == "item.started" || kind == "item.completed") && item_type == Some(&json!("mcp_tool_call")) {
            if item.get("server") != Some(&json!("grainy")) {
                return Ok(vec![Event::new("error", "Unexpected MCP server outside Grainy", None)]);
            }
            let event =
                tool_event(item.get("tool").unwrap_or(&Value::Null), item.get("arguments").unwrap_or(&Value::Null));
            return Ok(self.once(item.get("id"), event));
        }
        if kind == "error" || kind == "turn.failed" {
            return Ok(self.error(&Value::Object(data.clone())));
        }
        if kind == "turn.completed" {
            self.succeeded = true;
        }
        Ok(Vec::new())
    }

    /// Port of `EventParser.gemini`.
    pub fn gemini(&mut self, data: &Value) -> Decoded {
        let data = data.as_object().ok_or(Undecodable)?;
        let kind = data.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "init" && truthy(data.get("session_id")) {
            return Ok(vec![Event::new("session", text_of(data, "session_id"), None)]);
        }
        if kind == "message" && data.get("role") == Some(&json!("assistant")) {
            return Ok(vec![Event::new("text", text_of(data, "content"), None)]);
        }
        if kind == "tool_use" {
            let event = tool_event(
                data.get("tool_name").unwrap_or(&Value::Null),
                data.get("parameters").unwrap_or(&Value::Null),
            );
            return Ok(self.once(data.get("tool_id"), event));
        }
        if kind == "error" {
            return Ok(self.error(&Value::Object(data.clone())));
        }
        if kind == "result" {
            self.failed = self.failed || data.get("status") != Some(&json!("success"));
            self.succeeded = !self.failed;
            if self.failed {
                return Ok(self.error(&Value::Object(data.clone())));
            }
        }
        Ok(Vec::new())
    }
}

/// Builds a program's arguments (and writes its run files).
pub type Build = fn(&Request, &dyn System) -> Result<Vec<String>, BuildError>;
/// Decodes one stream line.
pub type Parse = fn(&mut EventParser, &Value) -> Decoded;
/// Finds the installed program.
pub type Detect = fn(&dyn System) -> Option<Vec<String>>;

/// Port of `Profile`: one AI program.
#[derive(Clone, Copy)]
pub struct Profile {
    /// `claude`, `codex`, `gemini`.
    pub key: &'static str,
    /// The name shown (through `tr`).
    pub name: &'static str,
    /// Finds the program.
    pub detect: Detect,
    /// Its arguments.
    pub build: Build,
    /// Its stream decoder.
    pub parse: Parse,
    /// How a conversation continues (documentation, as the oracle keeps it).
    pub resume: &'static str,
    /// The program keeps the conversation itself (no earlier turns are sent again).
    pub persistent: bool,
    /// The arguments of its sign-in status command, if it has one.
    pub status_args: Option<&'static [&'static str]>,
    /// The arguments of its sign-in command.
    pub login_args: &'static [&'static str],
    /// Exit codes that mean "sign in again".
    pub auth_exit_codes: &'static [i32],
}

impl Profile {
    /// Port of `installer(platform)`.
    pub fn installer(&self, os: Os) -> Option<&'static Installer> {
        INSTALLERS.iter().find(|(key, platform, _)| *key == self.key && *platform == os).map(|(_, _, i)| i)
    }
}

impl std::fmt::Debug for Profile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Profile").field("key", &self.key).finish()
    }
}

fn detect_claude(sys: &dyn System) -> Option<Vec<String>> {
    discover::find_program(sys, "claude")
}

fn detect_gemini(sys: &dyn System) -> Option<Vec<String>> {
    discover::find_program(sys, "gemini")
}

/// Port of `PROFILES`.
pub const PROFILES: [Profile; 3] = [
    Profile {
        key: "claude",
        name: "Claude Code",
        detect: detect_claude,
        build: claude_command,
        parse: EventParser::claude,
        resume: "its own session (--resume)",
        persistent: true,
        status_args: Some(&["auth", "status", "--json"]),
        login_args: &["auth", "login"],
        auth_exit_codes: &[],
    },
    Profile {
        key: "codex",
        name: "Codex",
        detect: discover::codex_program,
        build: codex_command,
        parse: EventParser::codex,
        resume: "in-memory context (--ephemeral)",
        persistent: false,
        status_args: Some(&["login", "status"]),
        login_args: &["login"],
        auth_exit_codes: &[],
    },
    Profile {
        key: "gemini",
        name: "Gemini CLI",
        detect: detect_gemini,
        build: gemini_command,
        parse: EventParser::gemini,
        resume: "--resume <session id>",
        persistent: true,
        status_args: None,
        login_args: &[],
        auth_exit_codes: &[41],
    },
];

/// Port of `redact(text, secrets)`: the given secrets, bearer tokens, API keys and 64-hex keys become `[redacted]`.
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut text = text.to_owned();
    for secret in secrets {
        if !secret.is_empty() {
            text = text.replace(secret.as_str(), "[redacted]");
        }
    }
    static PATTERN: std::sync::OnceLock<regex_lite::Regex> = std::sync::OnceLock::new();
    let pattern = PATTERN.get_or_init(|| {
        regex_lite::Regex::new(r"(?i)(Bearer\s+)\S+|\b(?:sk-|sk-ant-)[A-Za-z0-9_-]+|\b[0-9a-f]{64}\b")
            .expect("valid pattern")
    });
    pattern.replace_all(&text, "[redacted]").into_owned()
}
