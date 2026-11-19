//! The servers end to end: the stdio loop (initialize, tools/list, tools/call with a job that the call waits for, a
//! preview) on a headless host with threads; the HTTP endpoint's checks and replies; the GUI-call queue's waiting
//! rules (the oracle's three GuiCall tests); the bridge files and their owner lock.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use grainy_catalog::Catalog;
use grainy_mcp::bridge::Bridge;
use grainy_mcp::gui::GuiQueue;
use grainy_mcp::headless::{Headless, Mode};
use grainy_mcp::host::CallError;
use grainy_mcp::service::{Config, McpService};
use grainy_mcp::{dumps, GrainyTools, Gui, Protocol};
use serde_json::{json, Value};

fn fresh(name: &str) -> PathBuf {
    let base = grainy_testkit::root().join("target").join("mcp-tests");
    std::fs::create_dir_all(&base).unwrap();
    let dir = std::fs::canonicalize(&base).unwrap().join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    let text = std::fs::canonicalize(&dir).unwrap().to_string_lossy().into_owned();
    PathBuf::from(text.strip_prefix("\\\\?\\").unwrap_or(&text))
}

/// A library with one photo, its service and tools on a threaded headless host.
fn library(name: &str) -> (PathBuf, Arc<McpService>, Arc<Headless>, Arc<GrainyTools>) {
    let root = fresh(name);
    let photo = root.join("photos").join("garden.png");
    std::fs::create_dir_all(photo.parent().unwrap()).unwrap();
    std::fs::copy(grainy_testkit::root().join("golden/mcp/_files/garden.png"), &photo).unwrap();
    let catalog = Catalog::new(root.join("library")).unwrap();
    catalog.add(&photo.to_string_lossy(), None, Some(&json!({"width": 240, "height": 160})), false).unwrap();
    let mut config = Config::new(root.join("library"), root.join("settings"));
    config.pictures = root.join("Pictures").join("Grainy");
    let service = McpService::new(config);
    let headless = Headless::new(catalog, Mode::Threaded);
    let gui: Arc<dyn Gui> = headless.clone();
    let tools = Arc::new(GrainyTools::new(service.clone(), Some(gui)));
    (root, service, headless, tools)
}

fn tools_list_bytes() -> String {
    let path = grainy_testkit::root().join("golden/mcp/tools_list/case.json");
    let case: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    case["definitions_bytes"].as_str().unwrap().to_owned()
}

#[test]
fn stdio_server_loop_initialize_list_and_call() {
    let (_root, _service, _headless, tools) = library("stdio");
    let protocol = Protocol::new("0.6.8");
    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
               "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test"}}})
        .to_string(),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string(),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}).to_string(),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "list_photos", "arguments": {}}})
            .to_string(),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "auto_tone", "arguments": {"ids": [1]}}})
            .to_string(),
        json!({"jsonrpc": "2.0", "id": 5, "method": "tools/call",
               "params": {"name": "get_preview", "arguments": {"ids": [1], "size": 128}}})
        .to_string(),
        "{not json".to_owned(),
        String::new(),
        json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {"name": "undo_last_batch"}}).to_string(),
    ]
    .join("\n");
    let mut output = Vec::new();
    grainy_mcp::stdio::serve(input.as_bytes(), &mut output, &protocol, tools.as_ref()).unwrap();
    let lines: Vec<Value> =
        output.split(|b| *b == b'\n').filter(|l| !l.is_empty()).map(|l| serde_json::from_slice(l).unwrap()).collect();
    assert_eq!(lines.len(), 7, "one reply per request, none for the notification or the empty line");
    assert_eq!(lines[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "grainy");
    let listed = String::from_utf8(dumps(&lines[1]["result"]["tools"])).unwrap();
    assert_eq!(listed, tools_list_bytes(), "tools/list is the oracle's, byte for byte");
    let text = |line: &Value| -> Value {
        let content = line["result"]["content"].as_array().unwrap();
        serde_json::from_str(content.last().unwrap()["text"].as_str().unwrap()).unwrap()
    };
    assert_eq!(text(&lines[2])["total"], 1);
    let tone = text(&lines[3]);
    assert_eq!(lines[3]["result"]["isError"], false);
    assert_eq!(tone["changed"], 1, "auto_tone waited for its job and wrote the batch: {tone}");
    assert!(tone["batch"].as_str().unwrap().starts_with('b'));
    let preview = &lines[4]["result"]["content"];
    assert_eq!(preview[0]["type"], "image");
    assert_eq!(text(&lines[4])["previews"], json!([{"id": 1, "w": 128, "h": 85}]));
    assert_eq!(lines[5], json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "Parse error"}}));
    assert_eq!(text(&lines[6])["undone"], tone["batch"]);
}

fn request(port: u16, raw: &str) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    stream.write_all(raw.as_bytes()).unwrap();
    read_reply(&mut BufReader::new(stream))
}

fn read_reply(reader: &mut BufReader<TcpStream>) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let status: u16 = line.split_whitespace().nth(1).unwrap().parse().unwrap();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h).unwrap();
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (k, v) = h.split_once(':').unwrap();
        headers.push((k.trim().to_owned(), v.trim().to_owned()));
    }
    let length: usize = headers.iter().find(|(k, _)| k == "Content-Length").map_or(0, |(_, v)| v.parse().unwrap());
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    (status, headers, body)
}

fn post(port: u16, path: &str, token: Option<&str>, kind: &str, body: &str, host: Option<&str>) -> String {
    let host = host.map_or(format!("127.0.0.1:{port}"), String::from);
    let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
    format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\n{auth}Content-Type: {kind}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
}

#[test]
fn http_endpoint_checks_and_answers() {
    let (_root, service, _headless, tools) = library("http");
    let token = "0123456789abcdef".repeat(4);
    let endpoint = grainy_mcp::http::Endpoint {
        protocol: Protocol::new("0.6.8"),
        tools: tools.clone(),
        service: service.clone(),
        token: token.clone(),
    };
    let server = grainy_mcp::http::start(0, endpoint).unwrap();
    let port = server.port();
    let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26"}})
        .to_string();
    let json_kind = "application/json";
    let (status, headers, _) = request(port, &post(port, "/mcp", None, json_kind, &init, None));
    assert_eq!(status, 401);
    assert!(headers.contains(&("WWW-Authenticate".into(), "Bearer realm=\"Grainy\"".into())));
    assert_eq!(request(port, &post(port, "/other", Some(&token), json_kind, &init, None)).0, 404);
    assert_eq!(request(port, &post(port, "/mcp", Some(&token), json_kind, &init, Some("example.com"))).0, 403);
    assert_eq!(request(port, &post(port, "/mcp", Some(&token), "text/plain", &init, None)).0, 415);
    assert_eq!(request(port, &post(port, "/mcp", Some(&"f".repeat(64)), json_kind, &init, None)).0, 401);
    let (status, headers, _) = request(port, &format!("GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"));
    assert_eq!((status, headers.contains(&("Allow".into(), "POST".into()))), (405, true));
    assert_eq!(request(port, &post(port, "/mcp", Some(&token), json_kind, "{bad", None)).0, 400);
    let (status, _, body) = request(port, &post(port, "/mcp/stdio", Some(&token), json_kind, "{bad", None));
    assert_eq!(
        (status, serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"].clone()),
        (200, json!(-32700))
    );
    // Two requests on one kept-alive connection.
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    stream.write_all(post(port, "/mcp", Some(&token), json_kind, &init, None).as_bytes()).unwrap();
    let (status, headers, body) = read_reply(&mut reader);
    assert_eq!(status, 200);
    assert!(headers.contains(&("Keep-Alive".into(), "timeout=55".into())));
    assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["result"]["protocolVersion"], "2025-03-26");
    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "get_photo", "arguments": {"ids": [1]}}})
        .to_string();
    stream.write_all(post(port, "/mcp", Some(&token), json_kind, &call, None).as_bytes()).unwrap();
    let (status, _, body) = read_reply(&mut reader);
    let reply: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(status, 200);
    let text: Value = serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["photos"][0]["name"], "garden.png");
    drop(stream);
    // The server notes a request's activity after it has written the reply (the oracle's order), so the test
    // may read the reply before the line is there: wait for it.
    let noted = |tool: &str, result: &str| {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        loop {
            if service.activity().iter().any(|(_, name, outcome)| name == tool && outcome == result) {
                return true;
            }
            if std::time::Instant::now() > deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    assert!(noted("get_photo", "ok"));
    assert!(noted("http", "refused 401"));
    server.stop();
}

fn host_for_queue(name: &str) -> Arc<Headless> {
    let root = fresh(name);
    Headless::new(Catalog::new(root.join("library")).unwrap(), Mode::Deferred)
}

#[test]
fn gui_queue_started_call_returns_its_result_once_after_queue_timeout() {
    let host = host_for_queue("queue-started");
    let queue = Arc::new(GuiQueue::with_limits(Box::new(|| {}), 0.2, 8.0));
    let runs = Arc::new(AtomicUsize::new(0));
    let (q, r) = (queue.clone(), runs.clone());
    let caller = std::thread::spawn(move || {
        q.call(
            Box::new(move |_| {
                r.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(600));
            }),
            Instant::now() + Duration::from_secs(5),
        )
    });
    let until = Instant::now() + Duration::from_secs(5);
    while !caller.is_finished() && Instant::now() < until {
        host.with(|h| queue.pump(h));
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(caller.join().unwrap(), Ok(()));
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[test]
fn gui_queue_unstarted_call_is_cancelled_and_never_runs() {
    let host = host_for_queue("queue-unstarted");
    let queue = Arc::new(GuiQueue::with_limits(Box::new(|| {}), 0.2, 8.0));
    let runs = Arc::new(AtomicUsize::new(0));
    let (q, r) = (queue.clone(), runs.clone());
    let caller = std::thread::spawn(move || {
        q.call(
            Box::new(move |_| {
                r.fetch_add(1, Ordering::SeqCst);
            }),
            Instant::now() + Duration::from_secs(5),
        )
    });
    let outcome = caller.join().unwrap();
    assert!(matches!(&outcome, Err(CallError::Busy(t)) if t.contains("did not get to this call in time")));
    host.with(|h| queue.pump(h));
    assert_eq!(runs.load(Ordering::SeqCst), 0);
}

#[test]
fn gui_queue_unfinished_after_grace_and_closed_calls_are_busy() {
    let host = host_for_queue("queue-unfinished");
    let queue = Arc::new(GuiQueue::with_limits(Box::new(|| {}), 10.0, 0.2));
    let q = queue.clone();
    let caller = std::thread::spawn(move || {
        q.call(Box::new(|_| std::thread::sleep(Duration::from_secs(1))), Instant::now() + Duration::from_millis(200))
    });
    std::thread::sleep(Duration::from_millis(50));
    host.with(|h| queue.pump(h));
    assert_eq!(caller.join().unwrap(), Err(CallError::Unfinished));
    let q = queue.clone();
    let waiting = std::thread::spawn(move || q.call(Box::new(|_| {}), Instant::now() + Duration::from_secs(5)));
    std::thread::sleep(Duration::from_millis(100));
    queue.close();
    assert!(matches!(waiting.join().unwrap(), Err(CallError::Busy(t)) if t.contains("restarted")));
    assert!(matches!(queue.call(Box::new(|_| {}), Instant::now() + Duration::from_secs(5)), Err(CallError::Busy(_))));
}

#[test]
fn bridge_files_are_owned_by_one_process_at_a_time() {
    let root = fresh("bridge");
    let source = root.join("assets");
    std::fs::create_dir_all(&source).unwrap();
    std::fs::write(source.join("grainy-mcp-bridge.ps1"), "# bridge\n").unwrap();
    let protocol = Protocol::new("0.6.8");
    let mut first = Bridge::new(&root.join("settings"));
    assert!(first.claim() && first.owned());
    assert!(first.write_bridges(&source, &protocol));
    assert!(first.write_connection(47821, "ab"));
    let folder = first.folder().to_path_buf();
    assert_eq!(std::fs::read_to_string(folder.join("endpoint")).unwrap(), "http://127.0.0.1:47821/mcp/stdio");
    assert_eq!(std::fs::read_to_string(folder.join("auth-header")).unwrap(), "Authorization: Bearer ab");
    assert_eq!(std::fs::read_to_string(folder.join("grainy-mcp-bridge.ps1")).unwrap(), "# bridge\n");
    for (name, text) in protocol.offline_templates() {
        assert_eq!(std::fs::read_to_string(folder.join(name)).unwrap(), text);
    }
    let mut second = Bridge::new(&root.join("settings"));
    assert!(!second.claim(), "another owner holds the lock");
    assert!(folder.join("endpoint").exists(), "the owner's files stay");
    first.release();
    assert!(second.claim(), "free again after release");
    assert!(!folder.join("endpoint").exists(), "a new owner removes copies left behind");
    second.release();
    std::fs::write(folder.join("endpoint"), "stale").unwrap();
    let mut third = Bridge::new(&root.join("settings"));
    third.remove_stale();
    assert!(!folder.join("endpoint").exists() && !third.owned());
}
