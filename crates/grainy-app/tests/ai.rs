//! The AI connection and Ask AI (work package 30) against the oracle (`golden/app/ai_*`, made by
//! `tools/golden/gen_app_ai.py` with the Python Grainy's own `install_commands`, `activity_text`, `EventParser`,
//! `redact`, `parse_status`, `gemini_status`, `claude_command` / `codex_command` / `gemini_command`, `AiRunner._run`,
//! `find_program` / `find_codex`, `launch_install` / `launch_sign_in`, `McpService` and the `McpDialog` / `AiDialog`
//! windows driven headless), the tools through the window's host (the same replies and catalogue rows as grainy-mcp's
//! headless host, which is golden-tested against the oracle), and the windows (snapshots in Korean and English).
//!
//! Safety: every environment is a temporary one, programs are fake (scripted processes, recorded consoles), the
//! server is a fake binder (no socket), nothing reads the person's AI configuration or starts a real program.

mod common;

use std::collections::BTreeMap;
use std::io::{BufRead, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::snapshot::{pin_status, save, settle};
use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::ai::ask::{grainy_context, undo_line, undo_line_in, AskAi, Link, Seen};
use grainy_app::ai::auth::{authentication_failed, authentication_failed_value, gemini_status, parse_status};
use grainy_app::ai::connection::{
    activity_text, install_commands, install_text, Binder, Bound, Connection, Setup, Sources,
};
use grainy_app::ai::programs::{
    claude_command, codex_command, gemini_command, redact, tool_event, EventParser, Profile, Request, PROFILES,
};
#[cfg(windows)]
use grainy_app::ai::system::HostSystem;
use grainy_app::ai::system::{Console, Launch, Os, Piped, System};
use grainy_app::ai::{discover, install, runner, AiState};
use grainy_app::dialogs::mcp::{refresh, McpState, Shown};
use grainy_app::GrainyApp;
use grainy_i18n::Language;
use grainy_mcp::http::Endpoint;
use grainy_mcp::{GrainyTools, Tools};
use grainy_platform::json::Value as Json;
use grainy_platform::Preferences;
use grainy_testkit::Case;
use serde_json::{json, Value};

fn case(name: &str) -> Value {
    Case::open("app", name).json().clone()
}

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn language(code: &str) -> Language {
    if code == "en" {
        Language::English
    } else {
        Language::Korean
    }
}

/// A fresh folder named `name` (`common::temp_dir`: the process id is in the folder above), as a plain path.
fn fixed_dir(name: &str) -> PathBuf {
    let dir = common::temp_dir(name);
    let text = std::fs::canonicalize(&dir).unwrap().to_string_lossy().into_owned();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
}

/// The golden's paths below `<ROOT>` as this system's oracle writes them. The goldens were made on Windows, where
/// the oracle's paths read `\` (`\\` inside JSON text); on macOS its `pathlib` joins the same folders with `/`. Only
/// the path that follows `<ROOT>` or `<ROOT_JSON>` changes, up to its end (a quote, `;`, white space or the end of
/// the text): other backslashes (PowerShell's own `'Grainy\mcp\...'`, the test's `C:\p\claude.exe`) stay.
fn native_paths(value: &Value) -> Value {
    fn convert(text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        loop {
            let next = [("<ROOT_JSON>", r"\\"), ("<ROOT>", r"\")]
                .iter()
                .filter_map(|(marker, separator)| rest.find(marker).map(|at| (at, *marker, *separator)))
                .min_by_key(|(at, _, _)| *at);
            let Some((at, marker, separator)) = next else {
                out.push_str(rest);
                return out;
            };
            out.push_str(&rest[..at + marker.len()]);
            rest = &rest[at + marker.len()..];
            let end = rest.find(|c: char| matches!(c, '\'' | '"' | ';') || c.is_whitespace()).unwrap_or(rest.len());
            out.push_str(&rest[..end].replace(separator, "/"));
            rest = &rest[end..];
        }
    }
    match value {
        _ if cfg!(windows) => value.clone(),
        Value::String(s) => Value::String(convert(s)),
        Value::Array(a) => Value::Array(a.iter().map(native_paths).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), native_paths(v))).collect()),
        other => other.clone(),
    }
}

/// The golden's placeholders filled with `root`, its paths as this system's oracle writes them ([`native_paths`]).
fn unplace(value: &Value, root: &Path) -> Value {
    fn fill(value: &Value, root: &Path) -> Value {
        let root_text = root.to_string_lossy().into_owned();
        let json = serde_json::to_string(&root_text).unwrap();
        let uri = grainy_app::ai::programs::as_uri(root);
        match value {
            Value::String(s) => Value::String(
                s.replace("<ROOT_URI>", &uri)
                    .replace("<ROOT_JSON>", &json[1..json.len() - 1])
                    .replace("<ROOT>", &root_text),
            ),
            Value::Array(a) => Value::Array(a.iter().map(|v| fill(v, root)).collect()),
            Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), fill(v, root))).collect()),
            other => other.clone(),
        }
    }
    fill(&native_paths(value), root)
}

// ---- a fake computer ---------------------------------------------------------------------------------------------

struct ScriptedProcess {
    stdout: Option<Vec<u8>>,
    stderr: Option<Vec<u8>>,
    code: i32,
}

impl Piped for ScriptedProcess {
    fn stdout(&mut self) -> Option<Box<dyn BufRead + Send>> {
        self.stdout.take().map(|b| Box::new(Cursor::new(b)) as Box<dyn BufRead + Send>)
    }
    fn stderr(&mut self) -> Option<Box<dyn Read + Send>> {
        self.stderr.take().map(|b| Box::new(Cursor::new(b)) as Box<dyn Read + Send>)
    }
    fn wait(&mut self) -> std::io::Result<i32> {
        Ok(self.code)
    }
    fn stopper(&self) -> Arc<dyn Fn() + Send + Sync> {
        Arc::new(|| {})
    }
}

struct RecordedConsole;

impl Console for RecordedConsole {
    fn finished(&mut self) -> bool {
        false
    }
}

/// What the next program prints on stdout and stderr, its exit code, and whether it fails to start.
type Script = (Vec<u8>, Vec<u8>, i32, bool);

/// A computer with the given environment and home, scripted programs and recorded consoles. Launching a real
/// program is impossible through it.
struct FakeSystem {
    os: Os,
    env: BTreeMap<String, String>,
    script: Mutex<Option<Script>>,
    launched: Mutex<Vec<Launch>>,
    consoles: Mutex<Vec<Launch>>,
}

impl FakeSystem {
    fn new(os: Os, env: BTreeMap<String, String>) -> Self {
        Self { os, env, script: Mutex::new(None), launched: Mutex::new(Vec::new()), consoles: Mutex::new(Vec::new()) }
    }

    fn isolated(root: &Path, extra: &[(&str, String)]) -> Self {
        Self::new(Os::Windows, isolated_env(root, extra))
    }
}

fn isolated_env(root: &Path, extra: &[(&str, String)]) -> BTreeMap<String, String> {
    let at = |p: &str| root.join(p).to_string_lossy().into_owned();
    let mut env: BTreeMap<String, String> = [
        ("PATH", at("empty-path")),
        ("PATHEXT", ".COM;.EXE;.BAT;.CMD".to_owned()),
        ("LOCALAPPDATA", at("local")),
        ("APPDATA", at("roaming")),
        ("USERPROFILE", at("home")),
        ("HOME", at("home")),
        ("ProgramFiles", at("programs")),
        ("SystemRoot", "C:\\Windows".to_owned()),
        ("NoDefaultCurrentDirectoryInExePath", "1".to_owned()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    for (k, v) in extra {
        env.insert((*k).to_owned(), v.clone());
    }
    env
}

impl System for FakeSystem {
    fn os(&self) -> Os {
        self.os
    }
    fn var(&self, name: &str) -> Option<String> {
        self.env.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone())
    }
    fn vars(&self) -> Vec<(String, String)> {
        self.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }
    fn home(&self) -> PathBuf {
        PathBuf::from(self.var("USERPROFILE").unwrap_or_default())
    }
    fn piped(&self, launch: &Launch) -> std::io::Result<Box<dyn Piped>> {
        let script = self.script.lock().unwrap().clone().unwrap_or((Vec::new(), Vec::new(), 0, false));
        if script.3 {
            return Err(std::io::Error::other("cannot start"));
        }
        // Only programs that started are recorded (as the oracle's scripted `Popen` does).
        self.launched.lock().unwrap().push(launch.clone());
        Ok(Box::new(ScriptedProcess { stdout: Some(script.0), stderr: Some(script.1), code: script.2 }))
    }
    fn console(&self, launch: &Launch, _done: Option<PathBuf>) -> std::io::Result<Box<dyn Console>> {
        self.consoles.lock().unwrap().push(launch.clone());
        Ok(Box::new(RecordedConsole))
    }
    fn open_url(&self, _url: &str) -> bool {
        false
    }
}

// ---- texts -------------------------------------------------------------------------------------------------------

#[test]
fn install_commands_and_activity_texts_are_the_oracles() {
    let _turn = common::language_lock();
    let c = case("ai_texts");
    let root = fixed_dir("ai-texts");
    let sys = FakeSystem::isolated(&root, &[]);
    let standard = grainy_app::ai::connection::standard_folder(&sys, true);
    assert_eq!(
        standard.as_ref().map(|p| Value::String(p.to_string_lossy().into())),
        Some(unplace(&c["standard"], &root))
    );
    // The install texts take Windows folders (`Z:\Temp\it's lib\mcp`, `/tmp/...` read as `\tmp\...`) and compare
    // them without case (`os.path.normcase`), which only Windows does: on Windows only.
    let installs = if cfg!(windows) { c["installs"].as_array().unwrap().clone() } else { Vec::new() };
    let mut checked = 0;
    for item in &installs {
        let windows = item["windows"].as_bool().unwrap();
        let folder = unplace(&item["folder"], &root);
        let folder = folder.as_str().map(PathBuf::from);
        let standard = grainy_app::ai::connection::standard_folder(&sys, windows);
        let commands = install_commands(windows, folder.as_deref(), standard.as_deref());
        assert_eq!(commands, unplace(&item["commands"], &root), "{item}");
        for (key, expected) in unplace(&item["texts"], &root).as_object().unwrap() {
            assert_eq!(install_text(&commands[key]), text(expected), "{key}");
            checked += 1;
        }
    }
    assert_eq!(checked, if cfg!(windows) { 30 } else { 0 });
    for code in ["ko", "en"] {
        grainy_i18n::set_language(language(code));
        for row in c["activity"][code].as_array().unwrap() {
            assert_eq!(activity_text(text(&row[0]), text(&row[1])), text(&row[2]), "{row}");
        }
    }
    grainy_i18n::set_language(Language::Korean);
}

// ---- events ------------------------------------------------------------------------------------------------------

fn events_json(events: &[grainy_app::ai::programs::Event]) -> Value {
    Value::Array(events.iter().map(|e| json!([e.kind, e.text, e.count])).collect())
}

#[test]
fn streams_tools_redaction_and_sign_in_states_are_the_oracles() {
    let c = case("ai_events");
    for (name, stream) in c["streams"].as_object().unwrap() {
        let mut parser = EventParser::new();
        let parse: fn(&mut EventParser, &Value) -> _ = if name.starts_with("claude") {
            EventParser::claude
        } else if name.starts_with("codex") {
            EventParser::codex
        } else {
            EventParser::gemini
        };
        for (line, expected) in stream["lines"].as_array().unwrap().iter().zip(stream["events"].as_array().unwrap()) {
            let events = parse(&mut parser, line).expect("decodable");
            assert_eq!(events_json(&events), *expected, "{name}: {line}");
        }
        assert_eq!(parser.succeeded, stream["succeeded"].as_bool().unwrap(), "{name}");
        assert_eq!(parser.failed, stream["failed"].as_bool().unwrap(), "{name}");
        assert_eq!(parser.partial, stream["partial"].as_bool().unwrap(), "{name}");
    }
    for row in c["tools"].as_array().unwrap() {
        let e = tool_event(&row["name"], &row["arguments"]);
        assert_eq!(json!([e.kind, e.text, e.count]), row["event"], "{row}");
    }
    for row in c["redactions"].as_array().unwrap() {
        let secrets: Vec<String> = row["secrets"].as_array().unwrap().iter().map(|s| text(s).to_owned()).collect();
        assert_eq!(redact(text(&row["text"]), &secrets), text(&row["out"]), "{row}");
    }
    for row in c["authentication"].as_array().unwrap() {
        let got = match &row[0] {
            Value::String(s) => authentication_failed(s),
            other => authentication_failed_value(other),
        };
        assert_eq!(got, row[1].as_bool().unwrap(), "{row}");
    }
    for row in c["statuses"].as_array().unwrap() {
        let s = parse_status(
            text(&row["key"]),
            row["code"].as_i64().unwrap() as i32,
            text(&row["stdout"]),
            text(&row["stderr"]),
        );
        assert_eq!((s.state.as_str(), s.kind.as_str()), (text(&row["state"]), text(&row["kind"])), "{row}");
    }
    let root = fixed_dir("ai-gemini-status");
    for folder in ["with", "cli"] {
        std::fs::create_dir_all(root.join(folder).join(".gemini")).unwrap();
        std::fs::write(root.join(folder).join(".gemini").join("oauth_creds.json"), "{}").unwrap();
    }
    for row in c["gemini"].as_array().unwrap() {
        let environ = unplace(&row["environ"], &root);
        let env: BTreeMap<String, String> =
            environ.as_object().unwrap().iter().map(|(k, v)| (k.clone(), text(v).to_owned())).collect();
        let sys = FakeSystem::new(Os::Windows, env);
        let s = gemini_status(&sys, Some(&root.join(text(&row["home"]))));
        assert_eq!((s.state.as_str(), s.kind.as_str()), (text(&row["state"]), text(&row["kind"])), "{row}");
    }
}

// ---- program arguments ---------------------------------------------------------------------------------------------

fn tree_files(root: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, folder: &Path, out: &mut BTreeMap<String, String>) {
        let Ok(entries) = std::fs::read_dir(folder) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(base, &path, out);
            } else {
                let name = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(name, std::fs::read_to_string(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// The server entry in the oracle's key order (`{'command': ..., 'args': ...}`): the golden files sort their keys,
/// and the run files are written in insertion order.
fn server_order(server: &Value) -> Value {
    let Some(map) = server.as_object() else { return server.clone() };
    let mut out = serde_json::Map::new();
    for key in ["command", "args"] {
        if let Some(v) = map.get(key) {
            out.insert(key.to_owned(), v.clone());
        }
    }
    for (k, v) in map {
        if !out.contains_key(k) {
            out.insert(k.clone(), v.clone());
        }
    }
    Value::Object(out)
}

#[test]
fn program_arguments_and_run_files_are_the_oracles() {
    let c = case("ai_commands");
    let root = fixed_dir("ai-commands");
    let sys = FakeSystem::isolated(&root, &[("CODEX_HOME", root.join("codex-home").to_string_lossy().into_owned())]);
    for row in c["instructions"].as_array().unwrap() {
        let r = Request {
            executable: vec!["x".into()],
            workspace: root.clone(),
            server: json!({}),
            prompt: "p".into(),
            language: text(&row["language"]).into(),
            model: String::new(),
            session_id: String::new(),
            context: text(&row["context"]).into(),
        };
        assert_eq!(r.instruction(), text(&row["text"]));
    }
    assert_eq!(json!(grainy_app::ai::programs::CODEX_DISABLED.as_slice()), c["disabled"]);
    let npm = root.join("npm");
    for row in c["runs"].as_array().unwrap() {
        let name = text(&row["name"]);
        // The files the generator laid out before this run.
        match name {
            "cache" => {
                std::fs::create_dir_all(root.join("codex-home")).unwrap();
                std::fs::write(
                    root.join("codex-home").join("models_cache.json"),
                    r#"{"models": [{"slug": "gpt-5", "apply_patch_tool_type": "freeform", "shell_type": "local", "extra": [1, 2]}, {"slug": "mini", "tool_mode": "x"}]}"#,
                )
                .unwrap();
            }
            "bad-cache" => {
                std::fs::write(root.join("codex-home").join("models_cache.json"), r#"{"models": [1, 2]}"#).unwrap()
            }
            "no-storage" => {
                let dist = npm.join("node_modules").join("@google").join("gemini-cli").join("dist");
                std::fs::create_dir_all(&dist).unwrap();
                std::fs::write(dist.join("index.js"), "// entry").unwrap();
            }
            "plain" if text(&row["kind"]) == "gemini" => {
                let config =
                    npm.join("node_modules/@google/gemini-cli/node_modules/@google/gemini-cli-core/dist/src/config");
                std::fs::create_dir_all(&config).unwrap();
                std::fs::write(config.join("storage.js"), "// storage").unwrap();
            }
            _ => {}
        }
        let request = unplace(&row["request"], &root);
        let r = Request {
            executable: request["executable"].as_array().unwrap().iter().map(|v| text(v).to_owned()).collect(),
            workspace: PathBuf::from(text(&request["workspace"])),
            server: server_order(&request["server"]),
            prompt: text(&request["prompt"]).into(),
            language: text(&request["language"]).into(),
            model: text(&request["model"]).into(),
            session_id: text(&request["session_id"]).into(),
            context: text(&request["context"]).into(),
        };
        let built = match text(&row["kind"]) {
            "claude" => claude_command(&r, &sys),
            "codex" => codex_command(&r, &sys),
            _ => gemini_command(&r, &sys),
        };
        match built {
            Ok(argv) => assert_eq!(json!(argv), unplace(&row["argv"], &root), "{name}"),
            Err(error) => assert_eq!(error.to_string(), text(&row["error"]), "{name}"),
        }
        let files: Value = serde_json::to_value(tree_files(&r.workspace)).unwrap();
        assert_eq!(files, unplace(&row["files"], &root), "{name}");
    }
}

// ---- the runner --------------------------------------------------------------------------------------------------

#[test]
fn a_run_reports_the_oracles_events_and_result() {
    let c = case("ai_runner");
    let root = fixed_dir("ai-runner");
    let extra: Vec<(&str, String)> =
        c["env"].as_object().unwrap().iter().map(|(k, v)| (k.as_str(), text(v).to_owned())).collect();
    for row in c["scenarios"].as_array().unwrap() {
        let sys = FakeSystem::isolated(&root, &extra);
        *sys.script.lock().unwrap() = Some((
            text(&row["stdout"]).as_bytes().to_vec(),
            text(&row["stderr"]).as_bytes().to_vec(),
            row["code"].as_i64().unwrap() as i32,
            row["fail"].as_bool().unwrap(),
        ));
        let profile = PROFILES.iter().find(|p| p.key == text(&row["key"])).unwrap();
        let name = text(&row["name"]);
        let request = Request {
            executable: vec!["prog".into()],
            workspace: root.join("w").join(name),
            server: json!({}),
            prompt: "p".into(),
            language: "ko".into(),
            model: String::new(),
            session_id: String::new(),
            context: String::new(),
        };
        std::fs::create_dir_all(&request.workspace).unwrap();
        let secrets: Vec<String> = row["secrets"].as_array().unwrap().iter().map(|s| text(s).to_owned()).collect();
        let (events, result, session, diagnostic) = runner::run_here(
            &sys,
            profile,
            &request,
            vec!["prog".into(), "p".into()],
            &secrets,
            Duration::from_secs(30),
        );
        assert_eq!(events_json(&events), row["events"], "{name}");
        assert_eq!(result, text(&row["result"]), "{name}");
        assert_eq!(session, text(&row["session"]), "{name}");
        assert_eq!(diagnostic, text(&row["diagnostic"]), "{name}");
        let launched = sys.launched.lock().unwrap().clone();
        let started: Vec<Value> = launched
            .iter()
            .map(|l| {
                let env: serde_json::Map<String, Value> = l
                    .env
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(k, _)| ["CLAUDE_CODE", "MCP_", "GEMINI_CLI_SYSTEM"].iter().any(|p| k.starts_with(p)))
                    .map(|(k, v)| (k, Value::String(v)))
                    .collect();
                json!({"argv": l.argv, "cwd": l.cwd.to_string_lossy(), "env": env})
            })
            .collect();
        assert_eq!(Value::Array(started), unplace(&row["started"], &root), "{name}");
    }
}

// ---- discovery and launches ----------------------------------------------------------------------------------

/// The layouts are Windows installations (`claude.EXE`, `npm.CMD`, `PATHEXT`, `%LOCALAPPDATA%\Programs`), found by
/// this computer's own system ([`HostSystem`]): Windows only. On macOS the host system would look in this Mac's
/// `/opt/homebrew/bin` and `/usr/local/bin`; the macOS discovery is covered with a fake Mac
/// (`macos_sign_in_and_installers_open_terminal_scripts`).
#[cfg(windows)]
#[test]
fn programs_are_found_where_the_oracle_finds_them() {
    let c = case("ai_paths");
    for layout in c["layouts"].as_array().unwrap() {
        let name = text(&layout["name"]);
        let root = fixed_dir(&format!("ai-paths-{name}"));
        for file in layout["files"].as_array().unwrap() {
            let path = root.join(text(file));
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "x").unwrap();
        }
        let extra: Vec<(&str, String)> = unplace(&layout["env"], &root)
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| (if k == "PATH" { "PATH" } else { "CODEX_INSTALL_DIR" }, text(v).to_owned()))
            .collect();
        let sys = HostSystem { env: Some(isolated_env(&root, &extra)) };
        let found = json!({
            "claude": discover::find_program(&sys, "claude"),
            "gemini": discover::find_program(&sys, "gemini"),
            "codex": discover::codex_program(&sys),
            "node": discover::find_tool(&sys, "node"),
            "npm": discover::find_tool(&sys, "npm"),
            "node_ready": discover::node_ready(&sys),
            "folders": discover::tool_folders(&sys, Os::Windows).iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "install_path": discover::install_environment(&sys, Os::Windows).into_iter().find(|(k, _)| k == "PATH").map(|(_, v)| v),
        });
        assert_eq!(found, unplace(&layout["found"], &root), "{name}");
    }
}

#[test]
fn installers_and_sign_in_open_the_oracles_consoles() {
    let _turn = common::language_lock();
    let c = case("ai_launch");
    let root = fixed_dir("ai-launch");
    let record = |l: &Launch| {
        let path = l.env.as_ref().and_then(|e| e.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()));
        json!({"argv": l.argv, "cwd": l.cwd.to_string_lossy(), "path": path})
    };
    let strip = |calls: &Value| {
        Value::Array(
            calls
                .as_array()
                .unwrap()
                .iter()
                .map(|c| json!({"argv": c["argv"], "cwd": c["cwd"], "path": c["path"]}))
                .collect(),
        )
    };
    for row in c["install"].as_array().unwrap() {
        grainy_i18n::set_language(language(text(&row["language"])));
        let sys = FakeSystem::isolated(&root, &[]);
        let key = text(&row["key"]);
        let installer = if key == "node" {
            install::NODE_WINDOWS
        } else {
            install::INSTALLERS.iter().find(|(k, os, _)| *k == key && *os == Os::Windows).unwrap().2
        };
        let expected = &row["installer"];
        assert_eq!(installer.command, text(&expected["command"]));
        assert_eq!(installer.vendor, text(&expected["vendor"]));
        assert_eq!(installer.source, text(&unplace(&expected["source"], &root)));
        assert_eq!(installer.shell, text(&expected["shell"]));
        assert_eq!(installer.needs_node, expected["needs_node"].as_bool().unwrap());
        install::launch_install(&installer, &root.join("settings"), Os::Windows, &sys).unwrap();
        let calls: Vec<Value> = sys.consoles.lock().unwrap().iter().map(record).collect();
        assert_eq!(Value::Array(calls), unplace(&strip(&row["calls"]), &root), "{key}");
    }
    grainy_i18n::set_language(Language::Korean);
    for (key, table) in c["installers"].as_object().unwrap() {
        for (platform, expected) in table.as_object().unwrap() {
            let os = Os::parse(platform);
            let installer = install::INSTALLERS.iter().find(|(k, o, _)| k == key && *o == os).unwrap().2;
            assert_eq!(installer.name, text(&expected["name"]));
            assert_eq!(installer.command, text(&expected["command"]));
        }
    }
    for row in c["sign_in"].as_array().unwrap() {
        let sys = FakeSystem::isolated(&root, &[]);
        let profile = PROFILES.iter().find(|p| p.key == text(&row["key"])).unwrap();
        let exe = vec![format!("C:\\p\\{}.exe", profile.key)];
        grainy_app::ai::auth::launch_sign_in(profile, Some(&exe), &root.join("settings"), Os::Windows, &sys).unwrap();
        let calls: Vec<Value> = sys.consoles.lock().unwrap().iter().map(record).collect();
        assert_eq!(Value::Array(calls), unplace(&strip(&row["calls"]), &root), "{}", profile.key);
    }
    let sys = FakeSystem::isolated(&root, &[]);
    let errors = [
        grainy_app::ai::auth::launch_sign_in(&PROFILES[0], None, &root, Os::Windows, &sys).err().map(|e| e.to_string()),
        grainy_app::ai::auth::launch_sign_in(&PROFILES[0], Some(&["x".to_owned()]), &root, Os::Other, &sys)
            .err()
            .map(|e| e.to_string()),
        install::launch_install(&install::NODE_WINDOWS, &root, Os::Other, &sys).err().map(|e| e.to_string()),
    ];
    assert_eq!(json!(errors), c["errors"]);
    for (profile, expected) in PROFILES.iter().zip(c["profiles"].as_array().unwrap()) {
        assert_eq!(profile.key, text(&expected["key"]));
        assert_eq!(profile.name, text(&expected["name"]));
        assert_eq!(profile.resume, text(&expected["resume"]));
        assert_eq!(profile.persistent, expected["persistent"].as_bool().unwrap());
        assert_eq!(json!(profile.status_args), expected["status_args"]);
        assert_eq!(json!(profile.login_args), expected["login_args"]);
        assert_eq!(json!(profile.auth_exit_codes), expected["auth_exit_codes"]);
    }
}

/// macOS: sign-in and installers run in Terminal from a script the person sees (`launch_sign_in` / `launch_install`
/// with `platform='darwin'`): `open -a Terminal <script>`, the script touching `closed` when it ends, the installer
/// with the tool folders on `PATH` and a pause. Nothing is started: the computer is a fake that records consoles.
#[test]
fn macos_sign_in_and_installers_open_terminal_scripts() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let root = fixed_dir("ai-launch-mac");
    let sys = FakeSystem::new(Os::MacOs, isolated_env(&root, &[]));
    let settings = root.join("settings");
    let q = grainy_app::ai::auth::shell_quote;
    let opened = |folder: &str, name: &str| {
        let launch = sys.consoles.lock().unwrap().pop().expect("a console was opened");
        assert_eq!(&launch.argv[..3], ["open", "-a", "Terminal"]);
        assert_eq!(launch.argv.len(), 4);
        assert_eq!(launch.env, None, "Terminal keeps its own environment; the script sets PATH");
        let script = PathBuf::from(&launch.argv[3]);
        assert_eq!(script.file_name().unwrap(), name);
        let directory = script.parent().unwrap();
        assert_eq!(directory.parent().unwrap(), settings.join(folder));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&script).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let done = directory.join("closed");
        (std::fs::read_to_string(&script).unwrap(), q(&format!("touch {}", q(&done.to_string_lossy()))))
    };
    for profile in PROFILES.iter() {
        let exe = vec![format!("/Users/kim/.local/bin/{}", profile.key)];
        grainy_app::ai::auth::launch_sign_in(profile, Some(&exe), &settings, Os::MacOs, &sys).unwrap();
        let (text, touch) = opened("ai-login", "signin.command");
        let argv: Vec<String> =
            exe.iter().map(String::as_str).chain(profile.login_args.iter().copied()).map(q).collect();
        let home = q(&sys.home().to_string_lossy());
        let expected = format!(
            "#!/bin/sh\ntrap {touch} EXIT\ntrap 'exit 0' HUP INT TERM\ncd {home} || exit 1\n{}\n",
            argv.join(" ")
        );
        assert_eq!(text, expected, "{}", profile.key);
    }
    let path = std::iter::once(root.join("empty-path"))
        .chain(discover::tool_folders(&sys, Os::MacOs))
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(":");
    assert!(path.contains("/opt/homebrew/bin:/usr/local/bin:"), "{path}");
    let pause = q("결과를 확인한 뒤 아무 키나 누르면 이 설치 실행을 마칩니다.");
    let place = q(&settings.join("ai-install").to_string_lossy());
    for (key, _, installer) in install::INSTALLERS.iter().filter(|(_, os, _)| *os == Os::MacOs) {
        install::launch_install(installer, &settings, Os::MacOs, &sys).unwrap();
        let (text, touch) = opened("ai-install", "install.command");
        let expected = format!(
            "#!/bin/bash\nset -o pipefail\ntrap {touch} EXIT\ntrap 'exit 0' HUP INT TERM\nexport PATH={}\ncd {place} || exit 1\n(\n{}\n)\nresult=$?\nprintf '\\n%s\\n' {pause}\nread -r -n 1\nexit \"$result\"\n",
            q(&path),
            installer.command,
        );
        assert_eq!(text, expected, "{key}");
    }
    assert_eq!(
        install::INSTALLERS.iter().filter(|(_, os, _)| *os == Os::MacOs).map(|(_, _, i)| i.command).collect::<Vec<_>>(),
        [
            "curl -fsSL https://claude.ai/install.sh | bash",
            "curl -fsSL https://chatgpt.com/codex/install.sh | sh",
            "npm install -g @google/gemini-cli"
        ]
    );
}

// ---- the connection ------------------------------------------------------------------------------------------

/// A server that never opens a socket: ports in `taken` fail as a used port does.
#[derive(Clone, Default)]
struct FakeNet {
    taken: Arc<Mutex<Vec<u16>>>,
    bound: Arc<Mutex<Vec<u16>>>,
}

struct FakeServer(u16);

impl Bound for FakeServer {
    fn port(&self) -> u16 {
        self.0
    }
    fn stop(self: Box<Self>) {}
}

impl Binder for FakeNet {
    fn bind(&mut self, port: u16, _endpoint: Endpoint) -> std::io::Result<Box<dyn Bound>> {
        self.bound.lock().unwrap().push(port);
        if self.taken.lock().unwrap().contains(&port) {
            return Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "address in use"));
        }
        Ok(Box::new(FakeServer(port)))
    }
}

fn sequence<T: Clone + Send + 'static>(values: Vec<T>) -> Box<dyn FnMut() -> T + Send> {
    let values = Arc::new(Mutex::new(values));
    Box::new(move || values.lock().unwrap().remove(0))
}

fn test_setup(settings: &Path, library: &Path, net: &FakeNet, tokens: Vec<String>, ports: Vec<u16>) -> Setup {
    let counter = Arc::new(AtomicUsize::new(0));
    Setup {
        settings_folder: settings.to_path_buf(),
        library: library.to_path_buf(),
        pictures: PathBuf::from("P:\\Pictures\\Grainy"),
        version: "0.6.8".into(),
        binder: Box::new(net.clone()),
        sources: Sources {
            token: sequence(tokens),
            port: sequence(ports),
            service: Some((
                Box::new(move |_| Ok(format!("{:06x}", counter.fetch_add(1, Ordering::SeqCst) + 1))),
                Box::new(|| "12:00:00".to_owned()),
            )),
        },
        wake: Arc::new(|| {}),
    }
}

fn mcp_files(settings: &Path) -> Value {
    let folder = settings.join("mcp");
    let mut out = serde_json::Map::new();
    let Ok(entries) = std::fs::read_dir(&folder) else { return Value::Object(out) };
    let mut names: Vec<String> = entries.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    for name in names {
        let path = folder.join(&name);
        let value = if name == "endpoint" || name == "auth-header" {
            json!(std::fs::read_to_string(&path).unwrap())
        } else if name.ends_with(".ps1") || name.ends_with(".sh") || name.ends_with(".lock") {
            json!(std::fs::metadata(&path).unwrap().len())
        } else {
            json!("template")
        };
        out.insert(name, value);
    }
    Value::Object(out)
}

fn view(connection: &Connection, settings: &Path) -> Value {
    let preferences: Value = std::fs::read_to_string(settings.join("settings.json"))
        .ok()
        .map(|t| serde_json::from_str(&t).unwrap())
        .unwrap_or(Value::Null);
    json!({
        "preferences": preferences,
        "running": connection.running,
        "error": connection.error,
        "bound_port": connection.bound_port(),
        "port": connection.port(),
        "enabled": connection.enabled(),
        "token": connection.token(),
        "elsewhere": connection.elsewhere,
        "port_changed": connection.port_changed,
        "owner": connection.owned(),
        "activity": connection.service.activity().iter().map(|(a, b, c)| json!([a, b, c])).collect::<Vec<_>>(),
        "files": mcp_files(settings),
    })
}

#[test]
fn the_connection_starts_stops_and_writes_what_the_oracle_writes() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let c = case("ai_connection");
    let root = fixed_dir("ai-connection");
    let settings = root.join("settings");
    let net = FakeNet::default();
    let tokens = ["a", "b", "c", "d"].iter().map(|c| c.repeat(64)).collect();
    let ports = vec![40001, 47821, 40002, 40003, 41000, 41001, 41002];
    let mut connection = Connection::new(test_setup(&settings, &root.join("Library"), &net, tokens, ports));
    let steps = c["steps"].as_array().unwrap();
    let mut index = 0;
    let mut check = |connection: &Connection, returned: Value, bound: Vec<u16>| {
        let expected = unplace(&steps[index], &root);
        let mut got = view(connection, &settings);
        got["returned"] = returned;
        got["bound"] = json!(bound);
        for key in got.as_object().unwrap().keys() {
            assert_eq!(got[key], expected[key], "step {index} {}: {key}", expected["step"]);
        }
        index += 1;
    };
    let take = |net: &FakeNet| std::mem::take(&mut *net.bound.lock().unwrap());
    net.taken.lock().unwrap().push(47821);
    connection.start_if_enabled();
    check(&connection, Value::Null, take(&net));
    let r = connection.start(None);
    check(&connection, json!(r), take(&net));
    connection.stop();
    check(&connection, Value::Null, take(&net));
    let r = connection.start(None);
    check(&connection, json!(r), take(&net));
    connection.regenerate_token().unwrap();
    check(&connection, Value::Null, take(&net));
    let r = connection.change_port();
    check(&connection, json!(r), take(&net));
    let r = connection.set_enabled(false);
    check(&connection, json!(r), take(&net));
    std::fs::write(settings.join("mcp").join("endpoint"), "stale").unwrap();
    connection.start_if_enabled();
    check(&connection, Value::Null, take(&net));
    let r = connection.set_enabled(true);
    check(&connection, json!(r), take(&net));
    {
        let mut other = Connection::new(test_setup(&settings, &root.join("Library"), &net, vec![], vec![]));
        let r = other.start(None);
        check(&other, json!(r), take(&net));
        // Dropping a connection that never started leaves the owner's files alone.
    }
    connection.stop();
    let port = connection.port().unwrap();
    net.taken.lock().unwrap().push(port);
    take(&net);
    let r = connection.start(None);
    check(&connection, json!(r), take(&net));
    grainy_i18n::set_language(Language::English);
    let r = connection.start(None);
    check(&connection, json!(r), take(&net));
    grainy_i18n::set_language(Language::Korean);
    assert_eq!(index, steps.len());
    for (name, data) in grainy_app::ai::connection::BRIDGES {
        assert_eq!(json!(data.len()), c["bridges"][name], "{name}");
    }

    // A settings folder that cannot be written (it is a file).
    let broken = fixed_dir("ai-connection-broken");
    std::fs::write(broken.join("settings"), "not a folder").unwrap();
    let mut bad = Connection::new(test_setup(
        &broken.join("settings").join("inner"),
        &broken,
        &net,
        vec!["e".repeat(64)],
        vec![],
    ));
    for (step, expected) in ["start", "off", "on"].iter().zip(c["broken"].as_array().unwrap()) {
        let returned = match *step {
            "start" => bad.start(None),
            "off" => bad.set_enabled(false),
            _ => bad.set_enabled(true),
        };
        assert_eq!(json!(returned), expected["returned"], "{step}");
        assert_eq!(json!(bad.running), expected["running"], "{step}");
        assert_eq!(json!(bad.error), expected["error"], "{step}");
        let activity: Vec<Value> = bad.service.activity().iter().map(|(a, b, c)| json!([a, b, c])).collect();
        assert_eq!(json!(activity), expected["activity"], "{step}");
    }
}

// ---- the MCP dialog --------------------------------------------------------------------------------------------

struct FakeShown {
    prefs: serde_json::Map<String, Value>,
    running: bool,
    error: Option<String>,
    elsewhere: bool,
    port_changed: bool,
    bound_port: Option<u16>,
    folder: PathBuf,
    activity: Vec<(String, String, String)>,
}

impl Shown for FakeShown {
    fn enabled(&self) -> bool {
        self.prefs.get("mcp_enabled") != Some(&json!(false))
    }
    fn running(&self) -> bool {
        self.running
    }
    fn error(&self) -> Option<String> {
        self.error.clone()
    }
    fn elsewhere(&self) -> bool {
        self.elsewhere
    }
    fn port_changed(&self) -> bool {
        self.port_changed
    }
    fn bound_port(&self) -> Option<u16> {
        self.bound_port
    }
    fn port(&self) -> Option<u16> {
        self.prefs.get("mcp_port").and_then(Value::as_u64).filter(|p| (1024..=65535).contains(p)).map(|p| p as u16)
    }
    fn token(&self) -> Option<String> {
        self.prefs
            .get("mcp_token")
            .and_then(Value::as_str)
            .filter(|t| t.len() == 64 && t.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
            .map(str::to_owned)
    }
    fn export_root(&self) -> PathBuf {
        match self.prefs.get("mcp_export_folder").and_then(Value::as_str) {
            Some(s) if !s.is_empty() => PathBuf::from(s),
            _ => PathBuf::from("P:\\Pictures\\Grainy"),
        }
    }
    fn folder(&self) -> PathBuf {
        self.folder.clone()
    }
    fn activity(&self) -> Vec<(String, String, String)> {
        self.activity.clone()
    }
}

#[test]
fn the_mcp_dialog_shows_what_the_oracles_shows() {
    let _turn = common::language_lock();
    let c = case("ai_mcp_dialog");
    let root = fixed_dir("ai-mcp-dialog");
    let sys = FakeSystem::isolated(&root.join("elsewhere"), &[]);
    let entries = [
        None,
        Some(json!({"label": "AI 자동 톤", "count": 1234, "created": "2026-11-13T01:02:03"})),
        Some(json!({"label": "AI 보정", "count": 2, "created": "bad"})),
        None,
        None,
        Some(json!({"label": "AI 라벨", "count": 3, "created": "2026-01-02T03:04:05+00:00"})),
    ];
    let states = [
        (json!({"mcp_enabled": false}), false, None, false, false, None, false),
        (json!({"mcp_token": "e".repeat(64), "mcp_port": 47821}), true, None, false, false, Some(47821), false),
        (
            json!({"mcp_token": "e".repeat(64), "mcp_export_folder": "D:\\AI out"}),
            true,
            None,
            false,
            false,
            Some(41234),
            false,
        ),
        (
            json!({"mcp_port": 47821}),
            false,
            Some("포트 47821을(를) 쓸 수 없습니다: address in use"),
            false,
            false,
            None,
            false,
        ),
        (
            json!({}),
            false,
            Some("이 컴퓨터에서 실행 중인 다른 Grainy가 AI 연결을 쓰고 있습니다."),
            true,
            false,
            None,
            false,
        ),
        (json!({"mcp_token": "not a key", "mcp_port": 80}), true, None, false, true, Some(47821), true),
    ];
    let views = c["views"].as_array().unwrap();
    for (n, expected) in views.iter().enumerate() {
        grainy_i18n::set_language(language(text(&expected["language"])));
        let i = n % states.len();
        let (prefs, running, error, elsewhere, port_changed, bound_port, job) = states[i].clone();
        let activity = if i == 1 {
            vec![
                ("10:00:00".into(), "server".into(), "started".into()),
                ("10:01:00".into(), "apply_edits".into(), "ok".into()),
                ("10:02:00".into(), "auto_tone".into(), "job j1 done".into()),
            ]
        } else {
            Vec::new()
        };
        let shown = FakeShown {
            prefs: prefs.as_object().unwrap().clone(),
            running,
            error: error.map(str::to_owned),
            elsewhere,
            port_changed,
            bound_port,
            folder: root.join("mcp"),
            activity,
        };
        let mut state = McpState { key_visible: i == 2, ..McpState::default() };
        // The undo line's time is local (`astimezone()`): the golden was made at UTC+9 (03:04 UTC reads 12:04),
        // so the line is built in that zone, whatever zone this computer is in.
        let golden_zone = chrono::FixedOffset::east_opt(9 * 3600).unwrap();
        let undo = undo_line_in(entries[i].as_ref(), job, &golden_zone);
        let got = refresh(&mut state, &shown, &sys, undo);
        let name = text(&expected["name"]);
        assert_eq!(got.enabled, expected["enabled"].as_bool().unwrap(), "{name}");
        assert_eq!(got.status, text(&expected["status"]), "{name}");
        assert_eq!(state.advanced, expected["advanced"].as_bool().unwrap(), "{name}");
        assert_eq!(got.port_button, expected["port_button"].as_bool().unwrap(), "{name}");
        assert_eq!(got.port_note, expected["port_note"].as_bool().unwrap(), "{name}");
        assert_eq!(got.http, text(&expected["http"]), "{name}");
        assert_eq!(got.key, text(&expected["key"]), "{name}");
        assert_eq!(got.key_shown, expected["key_shown"].as_bool().unwrap(), "{name}");
        assert_eq!(got.show_key, text(&expected["show_key"]), "{name}");
        assert_eq!(got.key_enabled, expected["key_enabled"].as_bool().unwrap(), "{name}");
        assert_eq!(got.export_folder, text(&expected["export_folder"]), "{name}");
        let setups = unplace(&expected["setups"], &root);
        for ((key, _), (instruction, copied)) in grainy_app::ai::connection::CLIENTS.iter().zip(&got.setups) {
            assert_eq!(instruction, text(&setups[key][0]), "{name} {key}");
            assert_eq!(copied, text(&setups[key][1]), "{name} {key}");
        }
        assert_eq!(got.undo_label, text(&expected["undo_label"]), "{name}");
        assert_eq!(got.undo_enabled, expected["undo_enabled"].as_bool().unwrap(), "{name}");
        assert_eq!(json!(got.activity), expected["activity"], "{name}");
        assert_eq!(grainy_app::text::tr("AI 연결 (MCP)"), text(&expected["title"]));
    }
    grainy_i18n::set_language(Language::Korean);
}

// ---- the Ask AI window ---------------------------------------------------------------------------------------

struct FakeLink {
    preferences: Preferences,
    running: bool,
    folder: PathBuf,
    started: usize,
    bridges_ok: bool,
}

impl Link for FakeLink {
    fn preferences(&self) -> &Preferences {
        &self.preferences
    }
    fn save(&mut self, key: &str, value: Json) -> std::io::Result<()> {
        self.preferences.save(key, value)
    }
    fn enabled(&self) -> bool {
        !matches!(self.preferences.values.get("mcp_enabled"), Some(Json::Bool(false)))
    }
    fn running(&self) -> bool {
        self.running
    }
    fn error(&self) -> Option<String> {
        None
    }
    fn start(&mut self) -> bool {
        self.started += 1;
        self.running = true;
        true
    }
    fn set_enabled(&mut self, on: bool) -> bool {
        self.preferences.save("mcp_enabled", Json::Bool(on)).is_ok()
    }
    fn write_bridges(&mut self) -> bool {
        self.bridges_ok
    }
    fn write_connection(&mut self) -> bool {
        self.bridges_ok
    }
    fn folder(&self) -> PathBuf {
        self.folder.clone()
    }
    fn token(&self) -> Option<String> {
        Some("f".repeat(64))
    }
}

static BUILD_FAILS: AtomicBool = AtomicBool::new(false);

fn claude_found(_: &dyn System) -> Option<Vec<String>> {
    Some(vec!["C:\\p\\claude.exe".into()])
}
fn codex_missing(_: &dyn System) -> Option<Vec<String>> {
    None
}
fn gemini_found(_: &dyn System) -> Option<Vec<String>> {
    Some(vec!["node".into(), "C:\\g\\index.js".into()])
}
/// The oracle's test runner starts nothing: the arguments are not built (Gemini's would need its npm layout).
fn recorded(_: &Request, _: &dyn System) -> Result<Vec<String>, grainy_app::ai::programs::BuildError> {
    if BUILD_FAILS.load(Ordering::SeqCst) {
        return Err(grainy_app::ai::programs::BuildError::Io("no".into()));
    }
    Ok(vec!["prog".into()])
}

fn wait_idle(model: &AskAi) {
    let started = Instant::now();
    while model.runner.running() {
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn the_ask_ai_window_behaves_as_the_oracles() {
    let _turn = common::language_lock();
    let c = case("ai_dialog");
    for session in c["sessions"].as_array().unwrap() {
        let code = text(&session["language"]);
        grainy_i18n::set_language(language(code));
        let root = fixed_dir(&format!("ai-ask-{code}"));
        let sys: Arc<dyn System> = Arc::new(FakeSystem::isolated(&root, &[]));
        let mut link = FakeLink {
            preferences: Preferences::new(root.join("settings.json")),
            running: true,
            folder: root.join("mcp"),
            started: 0,
            bridges_ok: true,
        };
        let profiles: Vec<Profile> = PROFILES
            .iter()
            .map(|p| {
                let detect = match p.key {
                    "claude" => claude_found,
                    "codex" => codex_missing,
                    _ => gemini_found,
                };
                Profile { detect, build: recorded, ..*p }
            })
            .collect();
        let mut model = AskAi::new(&link, sys.clone(), profiles);
        let seen = Seen {
            folder: Some(("D:\\Photos\\여름".into(), true, 3)),
            selected: vec![7, 8],
            current: Some((7, "garden \"1\".png".into())),
        };
        let no_undo = || undo_line(None, false);
        let log = session["log"].as_array().unwrap();
        let mut at = 0;
        let mut snap = |model: &AskAi, link: &FakeLink, label: &str| {
            let expected = unplace(&log[at], &root);
            assert_eq!(text(&expected["label"]), label);
            let v = model.view(link, no_undo());
            let keys: Vec<&str> = model.profiles.iter().map(|p| p.key).collect();
            let per = |values: &[bool]| -> Value {
                Value::Object(keys.iter().zip(values).map(|(k, v)| ((*k).to_owned(), json!(v))).collect())
            };
            let got = json!({
                "notice": v.notice,
                "program": v.program,
                "programs": model.profiles.iter().map(|p| grainy_app::text::tr(p.name)).collect::<Vec<_>>(),
                "send": v.send,
                "stop": v.stop,
                "new": v.new,
                "enable_visible": v.enable_visible,
                "connection": v.connection,
                "status": model.status,
                "hints": v.hints,
                "hints_text": grainy_app::text::tr("프로그램이 없으면 설치…를 누르세요. 실행 전에 공식 설치 명령을 보여 줍니다."),
                "auth": Value::Object(keys.iter().zip(&model.auth_labels).map(|(k, l)| ((*k).to_owned(), json!(l))).collect()),
                "login_visible": per(&v.login_visible),
                "login_enabled": per(&v.login_enabled),
                "login_bold": per(&v.login_bold),
                "install_visible": per(&v.install_visible),
                "install_enabled": per(&v.install_enabled),
                "login_help": model.login_help,
                "transcript": model.transcript,
                "undo_label": v.undo_label,
                "undo_enabled": v.undo_enabled,
                "checks": model.checks,
                "preferences": std::fs::read_to_string(&link.preferences.path).ok().map(|t| serde_json::from_str::<Value>(&t).unwrap()),
            });
            for key in got.as_object().unwrap().keys() {
                assert_eq!(got[key], expected[key], "{code} {label}: {key}");
            }
            at += 1;
        };
        let started = session["started"].as_array().unwrap();
        let mut sent = 0;
        let mut run_keys: BTreeMap<String, String> = BTreeMap::new();
        let mut check_request = |model: &AskAi| {
            let expected = unplace(&started[sent], &root);
            let request = model.last_request.clone().expect("a request");
            assert_eq!(json!(request.executable), expected["executable"]);
            assert_eq!(request.server, expected["server"]);
            assert_eq!(request.prompt, text(&expected["prompt"]));
            assert_eq!(request.language, text(&expected["language"]));
            assert_eq!(request.model, text(&expected["model"]));
            assert_eq!(request.session_id, text(&expected["session_id"]));
            assert_eq!(request.context, text(&expected["context"]));
            let workspace = request.workspace.to_string_lossy().into_owned();
            let (prefix, run) = text(&expected["workspace"]).rsplit_once(std::path::MAIN_SEPARATOR).unwrap();
            let (head, key) = workspace.rsplit_once(std::path::MAIN_SEPARATOR).unwrap();
            assert_eq!(head, prefix);
            assert!(key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit()), "{key}");
            // The golden names the random run keys <RUN1>, <RUN2>...: the same name is the same folder.
            assert_eq!(run_keys.entry(run.to_owned()).or_insert_with(|| key.to_owned()).as_str(), key, "{run}");
            sent += 1;
        };
        let send = |model: &mut AskAi, link: &mut FakeLink| {
            wait_idle(model);
            model.send(link, &seen, no_undo());
        };
        snap(&model, &link, "opened");
        send(&mut model, &mut link);
        snap(&model, &link, "send before the notice");
        model.acknowledge(&mut link);
        snap(&model, &link, "notice acknowledged");
        model.auth_status_ready("claude", &grainy_app::ai::auth::AuthStatus::new("signed_in", "Claude"));
        model.auth_status_ready("gemini", &grainy_app::ai::auth::AuthStatus::new("timeout", ""));
        model.auth_status_ready("codex", &grainy_app::ai::auth::AuthStatus::new("signed_in", "ChatGPT"));
        snap(&model, &link, "statuses");
        model.input = "   ".into();
        send(&mut model, &mut link);
        snap(&model, &link, "empty request");
        model.input = "이 사진들 밝게 해 줘".into();
        send(&mut model, &mut link);
        check_request(&model);
        snap(&model, &link, "sent");
        for (kind, t, count) in [
            ("session", "sess-7", None),
            ("text", "밝게 ", None),
            ("text", "하겠습니다.", None),
            ("tool", "list_photos", None),
            ("tool", "apply_edits", Some(2)),
            ("error", "", None),
        ] {
            model.on_event(&grainy_app::ai::programs::Event::new(kind, t, count));
        }
        snap(&model, &link, "events");
        wait_idle(&model);
        model.on_finished("done");
        snap(&model, &link, "finished");
        model.model = "opus".into();
        model.choice_changed(&mut link);
        snap(&model, &link, "model changed (new conversation)");
        model.input = "a".repeat(8001);
        send(&mut model, &mut link);
        snap(&model, &link, "too long");
        model.input = "다시".into();
        send(&mut model, &mut link);
        check_request(&model);
        model.on_event(&grainy_app::ai::programs::Event::new("auth_error", "", None));
        wait_idle(&model);
        model.on_finished("auth_required");
        snap(&model, &link, "auth required");
        model.program = Some(2);
        model.choice_changed(&mut link);
        snap(&model, &link, "gemini chosen");
        model.input = "세 번째 요청".into();
        send(&mut model, &mut link);
        check_request(&model);
        wait_idle(&model);
        model.on_finished("stopped");
        model.input = "네 번째".into();
        send(&mut model, &mut link);
        check_request(&model);
        wait_idle(&model);
        model.on_event(&grainy_app::ai::programs::Event::new("exit", "3", None));
        model.on_finished("failed");
        snap(&model, &link, "second turn");
        model.on_finished("timeout");
        model.on_finished("other");
        snap(&model, &link, "other results");
        link.bridges_ok = false;
        model.input = "다섯".into();
        send(&mut model, &mut link);
        snap(&model, &link, "bridges missing");
        link.bridges_ok = true;
        BUILD_FAILS.store(true, Ordering::SeqCst);
        send(&mut model, &mut link);
        BUILD_FAILS.store(false, Ordering::SeqCst);
        snap(&model, &link, "launch failed");
        link.set_enabled(false);
        snap(&model, &link, "connection off");
        model.enable_connection(&mut link);
        snap(&model, &link, "connection on again");
        model.new_conversation();
        snap(&model, &link, "new conversation");
        model.detect_program(1);
        snap(&model, &link, "codex detected again (still missing)");
        assert_eq!(at, log.len());
        assert_eq!(sent, started.len());
        assert_eq!(grainy_context(&seen), text(&session["context"]));
        model.shutdown();
    }
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn programs_without_sessions_get_the_conversation_again() {
    // `send` for a program that keeps no conversation (Codex): the earlier turns go with the request, the oldest
    // dropped first when they are too long (AiDialog.send).
    // The transcript line is translated text: hold the language lock (another test may switch the language).
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let root = fixed_dir("ai-ask-history");
    let sys: Arc<dyn System> = Arc::new(FakeSystem::isolated(&root, &[]));
    let mut link = FakeLink {
        preferences: Preferences::new(root.join("settings.json")),
        running: true,
        folder: root.join("mcp"),
        started: 0,
        bridges_ok: true,
    };
    link.preferences.save("ai_notice_seen", Json::Bool(true)).unwrap();
    fn codex_found(_: &dyn System) -> Option<Vec<String>> {
        Some(vec!["codex.exe".into()])
    }
    let profiles = vec![Profile { detect: codex_found, build: recorded, ..PROFILES[1] }];
    let mut model = AskAi::new(&link, sys, profiles);
    let seen = Seen::default();
    model.input = "first".into();
    model.send(&mut link, &seen, undo_line(None, false));
    assert_eq!(model.last_request.as_ref().unwrap().prompt, "first");
    model.on_event(&grainy_app::ai::programs::Event::new("text", "ok", None));
    wait_idle(&model);
    model.on_finished("done");
    model.input = "second".into();
    model.send(&mut link, &seen, undo_line(None, false));
    assert_eq!(
        model.last_request.as_ref().unwrap().prompt,
        "Previous conversation, for context only; do not repeat its completed actions:\n[{\"role\": \"user\", \"content\": \"first\"}, {\"role\": \"assistant\", \"content\": \"ok\", \"status\": \"done\"}]\n\nNew request:\nsecond"
    );
    assert!(model.transcript.contains(&grainy_app::text::tr("기록 저장을 끈 새 실행에 이전 대화 내용을 전달합니다.")));
    wait_idle(&model);
    model.on_finished("done");
    // Only the last three turns are kept, and the context shrinks two entries at a time past 12,000 bytes.
    model.input = "x".repeat(4000);
    model.send(&mut link, &seen, undo_line(None, false));
    wait_idle(&model);
    model.on_event(&grainy_app::ai::programs::Event::new("text", "y".repeat(4000), None));
    model.on_finished("done");
    model.input = "last".into();
    model.send(&mut link, &seen, undo_line(None, false));
    let prompt = model.last_request.as_ref().unwrap().prompt.clone();
    let context = prompt.split_once('\n').unwrap().1.split_once("\n\nNew request:").unwrap().0;
    assert!(context.encode_utf16().count() * 2 <= 12000, "{}", context.len());
    assert!(!context.contains("\"first\""));
    wait_idle(&model);
    model.shutdown();
}

// ---- the window: the tools through its host ------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The window on a new library with two generated photos, the AI connection installed with a fake server, fixed
/// keys and ports and a temporary computer whose `LOCALAPPDATA` holds the settings folder (`<dir>\Grainy`), so the
/// install texts are the standard ones.
fn window<'a>(dir: &Path) -> Harness<'a, GrainyApp> {
    let photos = [dir.join("garden.png"), dir.join("garden two.png")];
    common::save_png(&common::garden(2, [1.0; 3], false), &photos[0]);
    common::save_png(&common::garden(2, [0.8, 1.0, 1.1], true), &photos[1]);
    let library = dir.join("Library");
    let data = dir.join("Grainy");
    let sys: Arc<dyn System> =
        Arc::new(FakeSystem::isolated(dir, &[("LOCALAPPDATA", dir.to_string_lossy().into_owned())]));
    let net = FakeNet::default();
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_library(&cc.egui_ctx, &library, Some(data));
        app.set_debug_overlay(false);
        let hook = Box::new(move |setup: Setup| {
            let tokens = vec!["9".repeat(64)];
            let mut fixed = test_setup(&setup.settings_folder, &setup.library, &net, tokens, vec![]);
            fixed.wake = setup.wake;
            fixed
        });
        grainy_app::ai::configure(&mut app.cx, sys, Some(hook));
        grainy_app::ai::install(&mut app.cx, false);
        app.open_paths(photos.to_vec());
        app
    });
    settle(&mut harness);
    harness
}

/// A tool call from another thread (as the HTTP server makes it) while the window runs its frames.
fn call(harness: &mut Harness<'_, GrainyApp>, tools: &Arc<GrainyTools>, name: &str, args: Value) -> Value {
    let (tools, name) = (tools.clone(), name.to_owned());
    let worker = std::thread::spawn(move || tools.call(&name, &args, false, Instant::now() + Duration::from_secs(45)));
    let started = Instant::now();
    while !worker.is_finished() {
        harness.step();
        std::thread::sleep(Duration::from_millis(2));
        assert!(started.elapsed() < Duration::from_secs(60), "the call did not finish");
    }
    let result = worker.join().unwrap().expect("tool result");
    settle(harness);
    result
}

fn reply_texts(result: &Value, scrub: &[(String, &str)]) -> Value {
    let mut out = Vec::new();
    for item in result["content"].as_array().unwrap() {
        let mut t = text(&item["text"]).to_owned();
        for (from, to) in scrub {
            t = t.replace(from.as_str(), to);
        }
        out.push(json!(t));
    }
    json!({"isError": result["isError"], "texts": out})
}

fn rows(db: &Path) -> Value {
    let catalog = grainy_catalog::Catalog::open_reader(db).unwrap();
    let photos: Vec<Value> = catalog
        .photos()
        .unwrap()
        .iter()
        .map(|p| json!({"id": p["id"], "rating": p["rating"], "flag": p["flag"], "label": p["label"], "settings": p["settings"]}))
        .collect();
    let history: Vec<Value> = [1, 2]
        .iter()
        .flat_map(|id| catalog.histories(*id).unwrap())
        .map(|h| json!([h["photo_id"], h["label"]]))
        .collect();
    json!({"photos": photos, "history": history})
}

#[test]
fn the_tools_act_on_the_window_as_on_the_headless_host() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = fixed_dir("ai-window-tools");
    let mut harness = window(&dir);
    let (service, gui) = {
        let state = harness.state_mut().cx.state::<AiState>();
        let connection = state.connection.as_ref().expect("the connection follows the library");
        assert!(connection.running, "{:?}", connection.error);
        assert_eq!(connection.bound_port(), Some(47821));
        (connection.service.clone(), connection.gui().unwrap())
    };
    assert_eq!(
        std::fs::read_to_string(dir.join("Grainy").join("mcp").join("endpoint")).unwrap(),
        "http://127.0.0.1:47821/mcp/stdio"
    );
    let app_tools = Arc::new(GrainyTools::new(service.clone(), Some(gui as Arc<dyn grainy_mcp::Gui>)));
    // The same library for the headless host: a copy of the catalogue as the import left it.
    let copy = dir.join("Copy");
    std::fs::create_dir_all(&copy).unwrap();
    {
        let catalog = grainy_catalog::Catalog::open_reader(dir.join("Library")).unwrap();
        catalog.backup(copy.join("catalog.sqlite")).unwrap();
    }
    let counter = Arc::new(AtomicUsize::new(0));
    let config = grainy_mcp::service::Config {
        directory: copy.clone(),
        settings_folder: dir.join("Grainy"),
        program_folders: vec![],
        pictures: PathBuf::from("P:\\Pictures\\Grainy"),
    };
    let headless_service = grainy_mcp::McpService::with_sources(
        config,
        Box::new(move |_| Ok(format!("{:06x}", counter.fetch_add(1, Ordering::SeqCst) + 1))),
        Box::new(|| "12:00:00".to_owned()),
    );
    let headless = grainy_mcp::headless::Headless::new(
        grainy_catalog::Catalog::new(&copy).unwrap(),
        grainy_mcp::headless::Mode::Inline,
    );
    let current = harness.state().cx.doc.current_id() as i64;
    headless.with(|h| {
        grainy_mcp::Host::activate(h, current).unwrap();
        h.select(&[current]);
    });
    let headless_tools = GrainyTools::new(headless_service, Some(headless.clone() as Arc<dyn grainy_mcp::Gui>));
    let scrub = [
        (serde_json::to_string(&dir.join("Library").to_string_lossy()).unwrap().trim_matches('"').to_owned(), "<LIB>"),
        (serde_json::to_string(&copy.to_string_lossy()).unwrap().trim_matches('"').to_owned(), "<LIB>"),
    ];
    let calls = [
        ("list_photos", json!({})),
        ("get_photo", json!({"ids": [1, 2]})),
        ("list_photos", json!({"scope": "selection"})),
        ("apply_edits", json!({"edits": [{"ids": [current], "set": {"exposure": 0.5}}]})),
        ("set_labels", json!({"ids": [2], "rating": 3, "label": "빨강"})),
        ("get_photo", json!({"ids": [current]})),
        ("undo_last_batch", json!({})),
        ("open_photo", json!({"id": 2})),
        ("list_photos", json!({"scope": "current"})),
    ];
    for (name, args) in calls {
        let from_window = call(&mut harness, &app_tools, name, args.clone());
        let from_headless = headless_tools.call(name, &args, false, Instant::now() + Duration::from_secs(45)).unwrap();
        headless.run_pending(None);
        assert_eq!(reply_texts(&from_window, &scrub), reply_texts(&from_headless, &scrub), "{name} {args}");
    }
    assert_eq!(rows(&dir.join("Library")), rows(&copy));
    // The window followed: photo 2 is open, the edit of photo 1 was undone and is shown so.
    assert_eq!(harness.state().cx.doc.current_id(), 2);
    let activity: Vec<String> = service.activity().iter().map(|(_, n, o)| format!("{n} {o}")).collect();
    assert!(activity.contains(&"server started".to_owned()), "{activity:?}");

    // A dialog in the way: writes wait (Busy), reads go on.
    harness.state_mut().cx.run("view.shortcuts");
    harness.run_steps(2);
    let busy = call(&mut harness, &app_tools, "set_labels", json!({"ids": [1], "rating": 1}));
    assert_eq!(busy["isError"], json!(true));
    assert!(text(&busy["content"][0]["text"]).contains("a dialog or menu is open"), "{busy}");
    harness.state_mut().cx.close_dialog("shortcuts");

    // The person's own undo (마지막 AI 작업 되돌리기): the last AI batch is undone, and noted.
    let applied = call(&mut harness, &app_tools, "set_labels", json!({"ids": [1], "rating": 5}));
    assert_eq!(applied["isError"], json!(false), "{applied}");
    let (label, enabled) = grainy_app::commands::ai::undo_line(&harness.state().cx);
    assert!(enabled && label.contains("1장"), "{label}");
    assert_eq!(grainy_app::commands::ai::undo_last(&mut harness.state_mut().cx), None);
    settle(&mut harness);
    let catalog = grainy_catalog::Catalog::open_reader(dir.join("Library")).unwrap();
    assert_eq!(catalog.photo(1).unwrap().unwrap()["rating"], json!(0));
    assert!(service.activity().iter().any(|(_, n, o)| n == "undo_last_batch" && o == "undone in Grainy"));

    // Switching the connection off stops the server and removes the files the bridges read.
    harness.state_mut().cx.state::<AiState>().connection.as_mut().unwrap().set_enabled(false);
    assert!(!dir.join("Grainy").join("mcp").join("endpoint").exists());
    let off = call(&mut harness, &app_tools, "get_photo", json!({"ids": [1]}));
    assert_eq!(off["isError"], json!(true));
    grainy_app::ai::shutdown(&mut harness.state_mut().cx);
}

// ---- snapshots ---------------------------------------------------------------------------------------------------

fn windows_snapshots(code: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language(code));
    let dir = fixed_dir(&format!("ai-window-{code}"));
    let mut harness = window(&dir);
    // The folder tree's open state depends on when the import finished: only the drives are open.
    common::snapshot::open_folders(harness.state_mut(), &[]);
    harness.state_mut().cx.run("ai.connection");
    settle(&mut harness);
    pin_status(harness.state_mut(), "");
    harness.run_steps(2);
    save(&mut harness, &format!("ai-connection-{code}"));
    harness.state_mut().cx.close_dialog("mcp");
    harness.state_mut().cx.run("ai.ask");
    settle(&mut harness);
    pin_status(harness.state_mut(), "");
    harness.run_steps(2);
    save(&mut harness, &format!("ai-ask-{code}"));
    grainy_app::ai::shutdown(&mut harness.state_mut().cx);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_ai_windows_korean() {
    windows_snapshots("ko");
}

#[test]
fn the_ai_windows_english() {
    windows_snapshots("en");
}
