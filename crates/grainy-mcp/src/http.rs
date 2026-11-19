//! Port of `luma/mcp_http.py`: the AI connection's HTTP endpoint on 127.0.0.1 only, a key on every request, bounded
//! in every phase. `POST /mcp` is the Streamable HTTP transport as Claude Code and Codex use it (JSON replies only, no
//! sessions, no event streams); `POST /mcp/stdio` serves the stdio bridges.
//!
//! Plain `std::net` with a thread per connection (at most [`MAX_CONNECTIONS`]), HTTP/1.1 with keep-alive.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::protocol::{Flavour, Protocol, DEADLINE_SECONDS};
use crate::service::McpService;
use crate::tools::{ToolResult, Tools};

/// `MAX_BODY`.
pub const MAX_BODY: usize = 1_048_576;
/// `MAX_CONNECTIONS`.
pub const MAX_CONNECTIONS: usize = 16;
/// `HEADER_SECONDS`: the first request on a connection, request line and headers.
pub const HEADER_SECONDS: u64 = 10;
/// `IDLE_SECONDS`: after a reply on a kept-alive connection, until the next request's headers are complete.
pub const IDLE_SECONDS: u64 = 60;
/// `BODY_SECONDS`.
pub const BODY_SECONDS: u64 = 10;
/// `CALL_SLOT_SECONDS`: waiting for one of the service's call slots.
pub const CALL_SLOT_SECONDS: f64 = 5.0;
/// `LINGER_SECONDS`.
pub const LINGER_SECONDS: u64 = 1;
/// `PATHS`.
pub const PATHS: [&str; 2] = ["/mcp", "/mcp/stdio"];
/// `SLOTS_BUSY`.
pub const SLOTS_BUSY: &str = "Grainy is handling other requests; nothing was changed. Try again in a moment.";

/// Port of `SlotTools`: the tools with one of the service's call slots held around each `tools/call`.
pub struct SlotTools<'a> {
    tools: &'a dyn Tools,
    service: &'a McpService,
}

impl Tools for SlotTools<'_> {
    fn definitions(&self) -> &[Value] {
        self.tools.definitions()
    }

    fn call(&self, name: &str, arguments: &Value, modern: bool, deadline: Instant) -> ToolResult {
        let wait = deadline.saturating_duration_since(Instant::now()).as_secs_f64().min(CALL_SLOT_SECONDS);
        if !self.service.calls.acquire(wait) {
            return Ok(serde_json::json!({"content": [{"type": "text", "text": SLOTS_BUSY}], "isError": true}));
        }
        let result = self.tools.call(name, arguments, modern, deadline);
        self.service.calls.release();
        result
    }
}

/// A running server (`McpHttpServer`).
pub struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    in_flight: Arc<AtomicUsize>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Server {
    /// The bound port.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Requests being handled now.
    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    /// Stop accepting connections and wait (bounded) for requests in flight.
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let until = Instant::now() + Duration::from_secs_f64(crate::service::STOP_SECONDS);
        while self.in_flight() > 0 && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// What a server answers with: the protocol, the tools and the key.
pub struct Endpoint {
    /// The protocol (server version, modern-era switch).
    pub protocol: Protocol,
    /// The tools (for the service's call slots).
    pub tools: Arc<dyn Tools + Send + Sync>,
    /// The service (call slots, activity log).
    pub service: Arc<McpService>,
    /// The 64 hex digits every request must carry as `Authorization: Bearer ...`.
    pub token: String,
}

/// Port of `start`: bind 127.0.0.1:`port` (0: any free port) and serve on a thread.
pub fn start(port: u16, endpoint: Endpoint) -> std::io::Result<Server> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    let port = listener.local_addr()?.port();
    let stop = Arc::new(AtomicBool::new(false));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let open = Arc::new(AtomicUsize::new(0));
    let endpoint = Arc::new(endpoint);
    let (stop2, flight2) = (stop.clone(), in_flight.clone());
    let thread = std::thread::Builder::new().name("GrainyMcpServer".into()).spawn(move || {
        for stream in listener.incoming() {
            if stop2.load(Ordering::SeqCst) {
                break;
            }
            let Ok(stream) = stream else { continue };
            if open.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                open.fetch_sub(1, Ordering::SeqCst);
                let _ = stream.shutdown(Shutdown::Both);
                continue;
            }
            let (endpoint, open, flight, stop) = (endpoint.clone(), open.clone(), flight2.clone(), stop2.clone());
            std::thread::spawn(move || {
                connection(stream, port, &endpoint, &flight, &stop);
                open.fetch_sub(1, Ordering::SeqCst);
            });
        }
    })?;
    Ok(Server { port, stop, in_flight, thread: Some(thread) })
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().rev().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// Request line and headers within `seconds`; `Err(Some(status))` for a malformed request.
fn read_head(reader: &mut BufReader<TcpStream>, seconds: u64) -> Result<Option<Request>, Option<u16>> {
    let until = Instant::now() + Duration::from_secs(seconds);
    let mut lines: Vec<String> = Vec::new();
    let mut total = 0usize;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(None);
        }
        let _ = reader.get_ref().set_read_timeout(Some(left));
        let mut line = Vec::new();
        match reader.by_ref().take(65537).read_until(b'\n', &mut line) {
            Ok(0) => return if lines.is_empty() { Ok(None) } else { Err(None) },
            Ok(n) => total += n,
            Err(_) => return Err(None),
        }
        if line.len() > 65536 || total > 65536 * 4 {
            return Err(Some(431));
        }
        let text = String::from_utf8_lossy(&line).trim_end_matches(['\r', '\n']).to_owned();
        if text.is_empty() {
            if lines.is_empty() {
                continue;
            }
            break;
        }
        lines.push(text);
        if lines.len() > 101 {
            return Err(Some(431));
        }
    }
    let first: Vec<&str> = lines[0].split_whitespace().collect();
    if first.len() != 3 || !first[2].starts_with("HTTP/1.") {
        return Err(Some(400));
    }
    let mut headers = Vec::new();
    for line in &lines[1..] {
        let Some((k, v)) = line.split_once(':') else { return Err(Some(400)) };
        headers.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    Ok(Some(Request { method: first[0].to_owned(), path: first[1].to_owned(), headers }))
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        411 => "Length Required",
        413 => "Payload Too Large",
        415 => "Unsupported Media Type",
        431 => "Request Header Fields Too Large",
        _ => "",
    }
}

fn write_reply(stream: &mut TcpStream, status: u16, headers: &[(&str, String)], body: &[u8]) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status} {}\r\nServer: Grainy\r\n", status_text(status));
    for (k, v) in headers {
        head += &format!("{k}: {v}\r\n");
    }
    head += "\r\n";
    stream.write_all(head.as_bytes())?;
    stream.write_all(body)?;
    stream.flush()
}

/// `early`: a reply before the body was read; the connection closes.
fn early(stream: &mut TcpStream, status: u16, extra: &[(&str, String)], service: &McpService) {
    let mut headers: Vec<(&str, String)> = extra.to_vec();
    headers.extend([
        ("Content-Length", "0".into()),
        ("Cache-Control", "no-store".into()),
        ("Connection", "close".into()),
    ]);
    let _ = write_reply(stream, status, &headers, b"");
    if status == 401 || status == 403 {
        service.note("http", &format!("refused {status}"));
    }
}

/// `shutdown_request`: a lingering close (half-close, then discard what the client still sends, bounded).
fn linger(stream: &mut TcpStream) {
    let _ = stream.shutdown(Shutdown::Write);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(LINGER_SECONDS)));
    let until = Instant::now() + Duration::from_secs(LINGER_SECONDS);
    let mut buffer = [0u8; 65536];
    let mut drained = 0;
    while drained <= 2 * MAX_BODY && Instant::now() < until {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => drained += n,
        }
    }
}

fn token_ok(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `hmac.compare_digest`: equal without an early exit.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn connection(stream: TcpStream, port: u16, endpoint: &Endpoint, in_flight: &AtomicUsize, stop: &AtomicBool) {
    let Ok(mut writer) = stream.try_clone() else { return };
    let mut reader = BufReader::new(stream);
    let mut seconds = HEADER_SECONDS;
    while !stop.load(Ordering::SeqCst) {
        let request = match read_head(&mut reader, seconds) {
            Ok(Some(r)) => r,
            Ok(None) | Err(None) => break,
            Err(Some(status)) => {
                let headers = [("Content-Length", "0".to_owned()), ("Connection", "close".to_owned())];
                let _ = write_reply(&mut writer, status, &headers, b"");
                break;
            }
        };
        if !route(&mut reader, &mut writer, &request, port, endpoint, in_flight) {
            break;
        }
        seconds = IDLE_SECONDS;
    }
    linger(&mut writer);
}

/// Port of `Handler.route`; false when the connection closes.
fn route(
    reader: &mut BufReader<TcpStream>,
    writer: &mut TcpStream,
    request: &Request,
    port: u16,
    endpoint: &Endpoint,
    in_flight: &AtomicUsize,
) -> bool {
    let service = &endpoint.service;
    let path = request.path.split('?').next().unwrap_or("");
    if !PATHS.contains(&path) {
        early(writer, 404, &[], service);
        return false;
    }
    let host = request.header("Host").unwrap_or("").to_lowercase();
    if ![format!("127.0.0.1:{port}"), format!("localhost:{port}"), format!("[::1]:{port}")].contains(&host) {
        early(writer, 403, &[], service);
        return false;
    }
    if let Some(origin) = request.header("Origin") {
        if origin != format!("http://127.0.0.1:{port}") && origin != format!("http://localhost:{port}") {
            early(writer, 403, &[], service);
            return false;
        }
    }
    if request.method != "POST" {
        early(writer, 405, &[("Allow", "POST".into())], service);
        return false;
    }
    let challenge = [("WWW-Authenticate", "Bearer realm=\"Grainy\"".to_owned())];
    if !token_ok(&endpoint.token) {
        early(writer, 401, &challenge, service);
        return false;
    }
    let authorization = request.header("Authorization").unwrap_or("");
    let (scheme, value) = authorization.split_once(' ').unwrap_or((authorization, ""));
    if !scheme.eq_ignore_ascii_case("bearer") || !same(value.trim().as_bytes(), endpoint.token.as_bytes()) {
        early(writer, 401, &challenge, service);
        return false;
    }
    let Some(length) = request.header("Content-Length") else {
        early(writer, 411, &[], service);
        return false;
    };
    let Ok(size) = length.trim().parse::<i64>() else {
        early(writer, 400, &[], service);
        return false;
    };
    if size < 0 {
        early(writer, 400, &[], service);
        return false;
    }
    if size as usize > MAX_BODY {
        early(writer, 413, &[], service);
        return false;
    }
    let kind = request.header("Content-Type").unwrap_or("").split(';').next().unwrap_or("").trim().to_lowercase();
    if kind != "application/json" {
        early(writer, 415, &[], service);
        return false;
    }
    let _ = reader.get_ref().set_read_timeout(Some(Duration::from_secs(BODY_SECONDS)));
    let mut body = vec![0u8; size as usize];
    if reader.read_exact(&mut body).is_err() {
        return false;
    }
    let close = request.header("Connection").is_some_and(|c| c.eq_ignore_ascii_case("close"));
    let flavour = if path == "/mcp/stdio" { Flavour::Stdio } else { Flavour::Http };
    let message = std::str::from_utf8(&body).ok().and_then(|t| serde_json::from_str::<Value>(t).ok());
    let reply = |writer: &mut TcpStream, status: u16, body: Option<&Value>| {
        let payload = body.map(crate::pyjson::dumps).unwrap_or_default();
        let mut headers: Vec<(&str, String)> = Vec::new();
        if !payload.is_empty() {
            headers.push(("Content-Type", "application/json".into()));
        }
        headers.push(("Content-Length", payload.len().to_string()));
        headers.push(("Cache-Control", "no-store".into()));
        headers.push(if close { ("Connection", "close".into()) } else { ("Keep-Alive", "timeout=55".into()) });
        write_reply(writer, status, &headers, &payload).is_ok()
    };
    let Some(message) = message else {
        let status = if flavour == Flavour::Stdio { 200 } else { 400 };
        return reply(writer, status, Some(&crate::stdio::parse_error())) && !close;
    };
    let deadline = Instant::now() + Duration::from_secs(DEADLINE_SECONDS);
    let started = Instant::now();
    in_flight.fetch_add(1, Ordering::SeqCst);
    let tools = SlotTools { tools: endpoint.tools.as_ref(), service };
    let outcome = endpoint.protocol.handle(&message, &request.headers, flavour, &tools, deadline);
    let sent = reply(writer, outcome.status, outcome.body.as_ref());
    in_flight.fetch_sub(1, Ordering::SeqCst);
    if outcome.status != 202 {
        note(service, &message, outcome.body.as_ref(), started);
    }
    sent && !close
}

/// Port of `Handler.note`: one line of recent activity per answered request.
fn note(service: &McpService, message: &Value, body: Option<&Value>, _started: Instant) {
    let mut name = message.get("method").and_then(Value::as_str).unwrap_or("request").to_owned();
    if name == "tools/call" {
        if let Some(tool) = message.get("params").and_then(|p| p.get("name")).and_then(Value::as_str) {
            name = tool.chars().take(64).collect();
        }
    }
    let result = match body {
        Some(b) if b.get("error").is_some() => format!("error {}", crate::pyjson::py_str(&b["error"]["code"])),
        Some(b) if b.get("result").and_then(|r| r.get("isError")).and_then(Value::as_bool) == Some(true) => {
            "error".into()
        }
        _ => "ok".into(),
    };
    service.note(&name.chars().take(64).collect::<String>(), &result);
}
