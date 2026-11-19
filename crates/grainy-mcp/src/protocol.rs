//! JSON-RPC 2.0 and the Model Context Protocol for Grainy's AI connection (port of `luma/mcp_protocol.py`). No
//! sockets: a parsed message and the request headers go in, a [`Reply`] comes out.
//!
//! Two eras: the legacy one (initialize handshake, protocol versions up to 2025-11-25) that today's clients speak,
//! and the modern one (2026-07-28: server/discover, per-request `_meta` and `Mcp-*` headers), built and tested but
//! switched off by [`MODERN_ENABLED`] until it has been tried with real clients.

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::{json, Map, Value};

use crate::pyjson::{dumps, py_str};
use crate::tools::{definitions, ToolResult, Tools};

/// `DEADLINE_SECONDS`: every request is answered within this plus the transport's grace period.
pub const DEADLINE_SECONDS: u64 = 45;
/// `MODERN_ENABLED`: turn on only after end-to-end runs with v2 clients.
pub const MODERN_ENABLED: bool = false;
/// `MODERN`.
pub const MODERN: [&str; 1] = ["2026-07-28"];
/// `LEGACY`.
pub const LEGACY: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// `SUPPORTED` (`MODERN + LEGACY`).
pub const SUPPORTED: [&str; 5] = ["2026-07-28", "2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];
/// `OFFLINE_VERSION`: the bridges' offline initialize (accepted by every TS SDK).
pub const OFFLINE_VERSION: &str = "2025-03-26";
/// `CACHE` ttlMs.
pub const CACHE_TTL_MS: i64 = 600_000;
/// `META_VERSION`.
pub const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
/// `META_CAPABILITIES`.
pub const META_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
/// `META_SERVER`.
pub const META_SERVER: &str = "io.modelcontextprotocol/serverInfo";
/// `INSTRUCTIONS`.
pub const INSTRUCTIONS: &str = concat!(
    "Grainy is a photo editor running on this computer; these tools drive its open window and catalog. ",
    "Edits are non-destructive: original files are never written, and every editing call is one batch that ",
    "undo_last_batch reverts, including set_labels (imports, saved files and virtual copies are outside undo). Work in few calls: target many photos at once (ids, a where filter, or scope ",
    "'selection'/'current'), give different values per photo in one apply_edits call, and prefer relative add values ",
    "(no need to read values first). Look at many photos with contact_sheet before using get_preview. Calls that ",
    "take long return a job id; poll it with get_job. If a call says Grainy is busy, nothing was changed: wait briefly ",
    "and retry or ask the person. After updating Grainy, start a new AI session: an existing session keeps its old tool list."
);
/// `OFFLINE_TEXT`: what the bridges answer while Grainy cannot be reached, in English and Korean.
pub const OFFLINE_TEXT: &str = concat!(
    "Grainy did not answer: it is not running on this computer, its AI connection is off, or it is still ",
    "starting or busy. Ask the person to start Grainy or, if it is already running, to open Settings > AI Connection ",
    "(MCP)… in Grainy, tick 'Turn on the AI connection' and look at the status shown there. Then try again; this AI ",
    "program does not need to be restarted. If you had asked for a change, check the photos before repeating it.\n",
    "Grainy가 응답하지 않았습니다. 이 컴퓨터에서 Grainy가 실행 중이 아니거나, AI 연결이 꺼져 있거나, 아직 시작 중이거나 바쁩니다. ",
    "Grainy를 실행하세요. 이미 실행 중이면 Grainy의 설정 › AI 연결 (MCP)…에서 'AI 연결 켜기'를 켜고 그 창의 상태를 확인하세요. ",
    "그런 다음 다시 시도하면 되며, AI 프로그램은 다시 시작하지 않아도 됩니다."
);
/// `OFFLINE_NOTE`: appended to the instructions of the offline initialize.
pub const OFFLINE_NOTE: &str = concat!(
    " When this session started Grainy did not answer (it was not running, or its AI connection was off); the ",
    "tools work as soon as it runs with the connection on, without restarting this program."
);
/// `OFFLINE_ERROR`: the bridges' own answer when even the offline files are missing.
pub const OFFLINE_ERROR: &str = concat!(
    "Grainy is not running, or its AI connection is off. Start Grainy, or turn its AI connection on ",
    "(Grainy: Settings > AI Connection (MCP)…). / Grainy가 실행 중이 아니거나 AI 연결이 꺼져 있습니다. Grainy를 실행하거나 ",
    "Grainy의 설정 › AI 연결 (MCP)…에서 AI 연결을 켜세요."
);

/// Port of `Reply`: an HTTP status, an optional JSON body and headers (`handle` never sets any).
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    /// HTTP status.
    pub status: u16,
    /// The JSON-RPC body, or `None` for a bodiless 202.
    pub body: Option<Value>,
    /// Extra response headers.
    pub headers: BTreeMap<String, String>,
}

impl Reply {
    fn new(status: u16, body: Value) -> Self {
        Self { status, body: Some(body), headers: BTreeMap::new() }
    }

    fn accepted() -> Self {
        Self { status: 202, body: None, headers: BTreeMap::new() }
    }

    /// The body as `dumps` writes it on the wire.
    pub fn body_bytes(&self) -> Option<Vec<u8>> {
        self.body.as_ref().map(dumps)
    }
}

/// Which endpoint a message arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    /// `/mcp`.
    Http,
    /// `/mcp/stdio`, the bridges: always the legacy era, every JSON-RPC error with HTTP status 200.
    Stdio,
}

/// Port of `error`: a JSON-RPC error body.
pub fn error(ident: Value, code: i64, message: &str, data: Option<Value>) -> Value {
    let mut body = Map::new();
    body.insert("code".into(), json!(code));
    body.insert("message".into(), json!(message));
    if let Some(data) = data {
        body.insert("data".into(), data);
    }
    json!({"jsonrpc": "2.0", "id": ident, "error": body})
}

/// Port of `result`: a JSON-RPC success body.
pub fn result(ident: Value, value: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": ident, "result": value})
}

/// Port of `valid_id`: a string or an integer (not a boolean, not a float).
pub fn valid_id(value: &Value) -> bool {
    value.is_string() || matches!(value, Value::Number(n) if n.is_i64() || n.is_u64())
}

/// Port of `header_value`: a header as sent; values of the form `=?base64?...?=` are decoded first (`None` when
/// the base64 or its UTF-8 is invalid).
pub fn header_value(value: Option<&str>) -> Option<String> {
    let value = value?;
    if value.starts_with("=?base64?") && value.ends_with("?=") {
        // Python slices [9:-2]: empty for the 10-character "=?base64?=".
        let inner = value.get(9..value.len().saturating_sub(2)).unwrap_or("");
        let bytes = crate::b64::decode(inner)?;
        return String::from_utf8(bytes).ok();
    }
    Some(value.to_string())
}

/// `header_value(header) == value` where `value` is any JSON value (Python `None == None` included).
fn header_equals(header: Option<String>, value: &Value) -> bool {
    match (&header, value) {
        (None, Value::Null) => true,
        (Some(h), Value::String(v)) => h == v,
        _ => false,
    }
}

/// The server: its version (for `serverInfo`) and the modern-era switch.
#[derive(Debug, Clone)]
pub struct Protocol {
    server_version: String,
    modern_enabled: bool,
}

impl Protocol {
    /// A server with the shipped switch ([`MODERN_ENABLED`]); `version` is the app's `__version__`.
    pub fn new(version: &str) -> Self {
        Self { server_version: version.to_string(), modern_enabled: MODERN_ENABLED }
    }

    /// The same with the modern era switched on or off (the oracle's tests monkeypatch `MODERN_ENABLED`).
    pub fn with_modern(mut self, enabled: bool) -> Self {
        self.modern_enabled = enabled;
        self
    }

    /// `SERVER_INFO`.
    pub fn server_info(&self) -> Value {
        json!({"name": "grainy", "title": "Grainy", "version": self.server_version})
    }

    /// Port of `initialize_result`.
    pub fn initialize_result(&self, version: Option<&str>, offline: bool) -> Value {
        let version = match version {
            Some(v) if LEGACY.contains(&v) => v,
            _ => LEGACY[0],
        };
        let instructions = format!("{INSTRUCTIONS}{}", if offline { OFFLINE_NOTE } else { "" });
        json!({"protocolVersion": version, "capabilities": {"tools": {"listChanged": false}},
               "serverInfo": self.server_info(), "instructions": instructions})
    }

    /// Port of `handle`: one JSON-RPC message (already parsed) to a [`Reply`]. `headers` are the request headers
    /// (names are matched case-insensitively; a repeated name keeps its last value).
    pub fn handle(
        &self,
        message: &Value,
        headers: &[(String, String)],
        flavour: Flavour,
        tools: &dyn Tools,
        deadline: Instant,
    ) -> Reply {
        let stdio = flavour == Flavour::Stdio;
        let lower: BTreeMap<String, String> = headers.iter().map(|(k, v)| (k.to_lowercase(), v.clone())).collect();
        let fail = |status: u16, ident: Value, code: i64, text: &str, data: Option<Value>| {
            Reply::new(if stdio { 200 } else { status }, error(ident, code, text, data))
        };

        let Some(message) = message.as_object() else { return fail(400, Value::Null, -32600, "Invalid Request", None) };
        let ident = message.get("id").filter(|v| valid_id(v)).cloned().unwrap_or(Value::Null);
        if message.get("jsonrpc") != Some(&json!("2.0")) {
            return fail(400, ident, -32600, "Invalid Request", None);
        }
        let Some(method) = message.get("method") else {
            if message.contains_key("result") || message.contains_key("error") {
                return Reply::accepted(); // a client's response
            }
            return fail(400, ident, -32600, "Invalid Request", None);
        };
        let Some(method) = method.as_str() else { return fail(400, ident, -32600, "Invalid Request", None) };
        let Some(raw_id) = message.get("id") else { return Reply::accepted() }; // notifications are ignored
        if !valid_id(raw_id) {
            return fail(400, Value::Null, -32600, "Invalid Request", None);
        }
        let params = match message.get("params") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => return fail(200, ident, -32602, "Invalid params", None),
        };
        if method == "initialize" {
            let version = params.get("protocolVersion").and_then(Value::as_str);
            return Reply::new(200, result(ident, self.initialize_result(version, false)));
        }
        if stdio {
            if method == "server/discover" {
                return Reply::new(200, error(ident, -32601, "Method not found", None));
            }
            return self.legacy(method, ident, &params, &lower, tools, deadline, false);
        }
        if !self.modern_enabled {
            // The legacy signal that makes a v2 client fall back to initialize; before any version header check.
            if method == "server/discover" {
                return Reply::new(200, error(ident, -32601, "Method not found", None));
            }
            return self.legacy(method, ident, &params, &lower, tools, deadline, true);
        }
        if let Some(Value::Object(meta)) = params.get("_meta") {
            if meta.contains_key(META_VERSION) {
                return self.modern(method, ident, &params, meta, &lower, tools, deadline);
            }
        }
        self.legacy(method, ident, &params, &lower, tools, deadline, true)
    }

    fn call(
        &self,
        ident: &Value,
        params: &Map<String, Value>,
        tools: &dyn Tools,
        deadline: Instant,
        modern: bool,
    ) -> Result<Value, Value> {
        let name = params.get("name");
        let known =
            |text: &str| tools.definitions().iter().any(|d| d.get("name").and_then(Value::as_str) == Some(text));
        let Some(name_text) = name.and_then(Value::as_str).filter(|text| known(text)) else {
            let shown = name.map_or_else(|| "None".to_string(), py_str);
            return Err(error(ident.clone(), -32602, &format!("Unknown tool: {shown}"), None));
        };
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => Value::Object(Map::new()),
            Some(value @ Value::Object(_)) => value.clone(),
            Some(_) => return Err(error(ident.clone(), -32602, "Invalid params: arguments must be an object", None)),
        };
        let outcome: ToolResult = tools.call(name_text, &arguments, modern, deadline);
        outcome.map_err(|_| error(ident.clone(), -32603, "Internal error", None))
    }

    #[allow(clippy::too_many_arguments)]
    fn legacy(
        &self,
        method: &str,
        ident: Value,
        params: &Map<String, Value>,
        headers: &BTreeMap<String, String>,
        tools: &dyn Tools,
        deadline: Instant,
        check: bool,
    ) -> Reply {
        if check {
            if let Some(version) = headers.get("mcp-protocol-version") {
                if !LEGACY.contains(&version.as_str()) {
                    return Reply::new(400, error(ident, -32600, "Unsupported MCP-Protocol-Version", None));
                }
            }
        }
        let fixed = match method {
            "ping" => Some(json!({})),
            "resources/list" => Some(json!({"resources": []})),
            "resources/templates/list" => Some(json!({"resourceTemplates": []})),
            "prompts/list" => Some(json!({"prompts": []})),
            _ => None,
        };
        if let Some(value) = fixed {
            return Reply::new(200, result(ident, value));
        }
        if method == "tools/list" {
            return Reply::new(200, result(ident, json!({"tools": tools.definitions()})));
        }
        if method == "tools/call" {
            return match self.call(&ident, params, tools, deadline, false) {
                Ok(value) => Reply::new(200, result(ident, value)),
                Err(failure) => Reply::new(200, failure),
            };
        }
        Reply::new(200, error(ident, -32601, "Method not found", None))
    }

    #[allow(clippy::too_many_arguments)]
    fn modern(
        &self,
        method: &str,
        ident: Value,
        params: &Map<String, Value>,
        meta: &Map<String, Value>,
        headers: &BTreeMap<String, String>,
        tools: &dyn Tools,
        deadline: Instant,
    ) -> Reply {
        let version = meta.get(META_VERSION).cloned().unwrap_or(Value::Null);
        let header = |name: &str| header_value(headers.get(name).map(String::as_str));
        if !header_equals(header("mcp-protocol-version"), &version) {
            return Reply::new(400, error(ident, -32020, "HeaderMismatch", None));
        }
        if !version.as_str().is_some_and(|v| MODERN.contains(&v)) {
            let data = json!({"supported": SUPPORTED, "requested": version});
            return Reply::new(400, error(ident, -32022, "Unsupported protocol version", Some(data)));
        }
        if !matches!(meta.get(META_CAPABILITIES), Some(Value::Object(_))) {
            return Reply::new(400, error(ident, -32602, "Invalid params: clientCapabilities", None));
        }
        if header("mcp-method").as_deref() != Some(method) {
            return Reply::new(400, error(ident, -32020, "HeaderMismatch", None));
        }
        if method == "tools/call" {
            let wanted = params.get("name").cloned().unwrap_or(Value::Null);
            if !header_equals(header("mcp-name"), &wanted) {
                return Reply::new(400, error(ident, -32020, "HeaderMismatch", None));
            }
        }
        let server_info = self.server_info();
        let complete = |ident: Value, value: Value| -> Reply {
            let Value::Object(mut map) = value else {
                return Reply::new(200, error(ident, -32603, "Internal error", None));
            };
            map.insert("resultType".into(), json!("complete"));
            map.insert("_meta".into(), json!({ (META_SERVER): server_info }));
            Reply::new(200, result(ident, Value::Object(map)))
        };
        let cache = |mut map: Map<String, Value>| -> Value {
            map.insert("ttlMs".into(), json!(CACHE_TTL_MS));
            map.insert("cacheScope".into(), json!("private"));
            Value::Object(map)
        };
        let with = |key: &str, value: Value| {
            let mut map = Map::new();
            map.insert(key.into(), value);
            cache(map)
        };
        match method {
            "server/discover" => {
                let mut map = Map::new();
                map.insert("supportedVersions".into(), json!(MODERN));
                map.insert("capabilities".into(), json!({"tools": {}}));
                map.insert("instructions".into(), json!(INSTRUCTIONS));
                complete(ident, cache(map))
            }
            "tools/list" => complete(ident, with("tools", json!(tools.definitions()))),
            "resources/list" => complete(ident, with("resources", json!([]))),
            "resources/templates/list" => complete(ident, with("resourceTemplates", json!([]))),
            "prompts/list" => complete(ident, with("prompts", json!([]))),
            "tools/call" => match self.call(&ident, params, tools, deadline, true) {
                Ok(value) => complete(ident, value),
                Err(failure) => Reply::new(200, failure),
            },
            // subscriptions/listen is not offered: it would never carry a notification (listChanged is false).
            _ => Reply::new(404, error(ident, -32601, "Method not found", None)),
        }
    }

    /// Port of `offline_templates`: the bridges' answers while Grainy is not reachable, one line each, with
    /// `__ID__` for the request id: `(file name, text)` in the oracle's order.
    pub fn offline_templates(&self) -> Vec<(String, String)> {
        let line = |value: Value| -> String {
            let text = String::from_utf8(dumps(&result(json!("__ID__"), value))).unwrap_or_default();
            text.replace("\"__ID__\"", "__ID__")
        };
        vec![
            ("offline-initialize.json".into(), line(self.initialize_result(Some(OFFLINE_VERSION), true))),
            ("offline-tools.json".into(), line(json!({"tools": definitions()}))),
            (
                "offline-call.json".into(),
                line(json!({"content": [{"type": "text", "text": OFFLINE_TEXT}], "isError": true})),
            ),
        ]
    }
}
