//! Feedback, updates, safe start and cloud backup (work package 31).
//!
//! Models against goldens made with the oracle's own code (`tools/golden/gen_app_feedback.py`,
//! `golden/app/feedback*`): `FeedbackDialog`, `UpdateController` (`startup`, `after_crash`, `install`),
//! `safe_start` and what `restore_folder_selection` asks for in safe mode, `cloud_backup` (`upload`, `surplus`,
//! `trim`, `digest`, `CloudBackup`, `dialog`) and `file_access.guide`. Scripted interaction without a window; the
//! real window (egui_kittest on wgpu) draws the dialogs in Korean and English.
//!
//! **Nothing leaves the machine.** The feedback post, the update feed, the download and the start of an installer
//! are traits ([`Transport`], the `UpdateOpener`, [`Backend`]); the tests give fakes that record what would have
//! been sent, and the defaults of the program fail with "not available in this build" (tested here too). The
//! recycle bin is a fake. Every file is in a temporary folder.

mod common;

use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::settings::{self, UpdateState};
use grainy_app::dialogs::about::MessageBox;
use grainy_app::dialogs::cloud_backup::{self, CloudBackupDialog, CloudBackupModel, CloudState, Uploader};
use grainy_app::dialogs::feedback::{self, FeedbackDialog, FeedbackModel, FeedbackState, Transport};
use grainy_app::dialogs::update::{self, Backend, Outcome, UpdateEnv};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::document::Document;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::startup::{self, Step};
use grainy_app::{AppContext, GrainyApp};
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_platform::feedback::{Gpu, Request, SendFailure, SystemFacts};
use grainy_platform::json::Value as Pref;
use grainy_platform::preferences::Preferences;
use grainy_platform::updater::Release;
use grainy_platform::user_paths::System;
use grainy_testkit::Case;
use regex_lite::Regex;
use serde_json::{json, Value};

fn turn() -> std::sync::MutexGuard<'static, ()> {
    common::language_lock()
}

fn set_language(code: &str) {
    grainy_i18n::set_language(Language::parse(code).unwrap());
}

fn context() -> AppContext {
    AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(Session::default()))
}

fn with_data(name: &str) -> (AppContext, PathBuf) {
    let folder = common::temp_dir(name);
    let mut cx = context();
    cx.set_data_folder(Some(folder.clone()));
    (cx, folder)
}

/// A context on a new, empty library of its own (a writable catalogue).
fn with_library(name: &str) -> (AppContext, PathBuf) {
    let folder = common::temp_dir(name);
    let mut cx = context();
    cx.set_document(Box::new(CatalogDocument::open(&folder.join("library")).expect("a library")));
    cx.set_data_folder(Some(folder.clone()));
    (cx, folder)
}

/// Runs the finished jobs until none is running.
fn drain(cx: &mut AppContext) {
    let started = Instant::now();
    loop {
        settings::pump(cx);
        if cx.jobs.idle() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
        assert!(started.elapsed() < Duration::from_secs(60), "a job did not finish");
    }
}

fn prefs_of(folder: &Path) -> Preferences {
    Preferences::new(folder.join("settings.json"))
}

fn text(value: &Value) -> String {
    value.as_str().unwrap_or("").to_owned()
}

fn clean(text: &str, folders: &[&Path]) -> String {
    let mut out = text.to_owned();
    for folder in folders {
        out = out.replace(&folder.to_string_lossy().to_string(), "<folder>");
    }
    let time = Regex::new(r"\d{4}-\d\d-\d\d[ T]\d\d:\d\d:\d\d").unwrap();
    let stamp = Regex::new(r"\d{8}-\d{6}").unwrap();
    stamp.replace_all(&time.replace_all(&out, "<time>"), "<stamp>").into_owned()
}

// ---- feedback ---------------------------------------------------------------------------------------------------

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                i += 2;
            }
            other => out.push(other),
        }
        i += 1;
    }
    String::from_utf8(out).unwrap()
}

fn form(body: &[u8]) -> BTreeMap<String, String> {
    String::from_utf8(body.to_vec())
        .unwrap()
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .map(|(k, v)| (percent_decode(k), percent_decode(v)))
        .collect()
}

/// A transport that records what would have been posted (message, system line, version) and answers from a queue.
struct Recorder {
    sent: Mutex<Vec<(String, String, String)>>,
    outcomes: Mutex<VecDeque<Result<u16, SendFailure>>>,
}

impl Recorder {
    fn new(outcomes: Vec<Result<u16, SendFailure>>) -> Arc<Self> {
        Arc::new(Self { sent: Mutex::new(Vec::new()), outcomes: Mutex::new(outcomes.into()) })
    }

    fn calls(&self) -> Vec<(String, String, String)> {
        self.sent.lock().unwrap().clone()
    }
}

impl Transport for Recorder {
    fn post(&self, request: &Request, timeout: u64) -> Result<u16, SendFailure> {
        assert_eq!(timeout, grainy_platform::feedback::TIMEOUT);
        assert!(request.url.starts_with("https://docs.google.com/forms/d/e/"), "{}", request.url);
        let form = form(&request.body);
        let entry = |key: &str| {
            let number = grainy_platform::feedback::FIELDS.iter().find(|(name, _)| *name == key).unwrap().1;
            form[&format!("entry.{number}")].clone()
        };
        self.sent.lock().unwrap().push((entry("message"), entry("system"), entry("version")));
        self.outcomes.lock().unwrap().pop_front().unwrap_or(Ok(200))
    }
}

fn facts_of(value: &Value) -> SystemFacts {
    let system = match value["system"].as_str().unwrap() {
        "mac" => System::MacOs,
        _ => System::Windows,
    };
    let adapter = text(&value["adapter"]);
    SystemFacts {
        system,
        mac_version: text(&value["mac_version"]),
        windows_release: text(&value["windows_release"]),
        windows_version: text(&value["windows_version"]),
        machine: text(&value["machine"]),
        gpu: (!adapter.is_empty()).then(|| Gpu { adapter, enabled: value["gpu_enabled"].as_bool().unwrap() }),
    }
}

/// The trailing `#XXXXX` references of the system lines, in order of first appearance.
fn references(calls: &[(String, String, String)]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for (_, system, _) in calls {
        let reference = system.rsplit('#').next().unwrap().to_owned();
        if !seen.contains(&reference) {
            seen.push(reference);
        }
    }
    seen
}

/// Our random references replaced by the golden's fixed ones, in order.
fn fixed(text: &str, seen: &[String], order: &[String]) -> String {
    let mut out = text.to_owned();
    for (random, golden) in seen.iter().zip(order) {
        out = out.replace(random, golden);
    }
    out
}

#[test]
fn the_feedback_window_follows_the_oracle() {
    let _turn = turn();
    let g = Case::open("app", "feedback_dialog").json().clone();
    let facts = facts_of(&g["facts"]);
    let order: Vec<String> = g["ref_order"].as_array().unwrap().iter().map(text_of).collect();
    for run in g["runs"].as_array().unwrap() {
        set_language(run["language"].as_str().unwrap());
        let (mut cx, folder) = with_data(&format!("feedback-run-{}", run["language"].as_str().unwrap()));
        let boom = || Err(SendFailure::Other("boom".into()));
        let recorder = Recorder::new(vec![boom(), Ok(200), Ok(200), boom(), Ok(200)]);
        {
            let state = cx.state::<FeedbackState>();
            state.transport = Some(recorder.clone());
            state.facts = Some(facts.clone());
            state.today = Some("2026-01-02".into());
        }
        let mut model = FeedbackModel::new(&cx);
        let texts = &run["texts"];
        assert_eq!(model.system(), text(&texts["system"]));
        let labels = texts["labels"].as_array().unwrap();
        // The Rust build names itself in what it sends (`rs-0.1.0`); otherwise the oracle's note.
        let note = text(&labels[1]).replace(grainy_platform::APP_VERSION, grainy_platform::BUILD_VERSION);
        assert_eq!(model.note(), note);
        assert_eq!(grainy_i18n::tr("피드백 보내기"), text(&texts["title"]));
        assert_eq!(
            grainy_i18n::tr(
                "문제점이나 개선 아이디어를 자유롭게 적어 주세요. 사진, 파일 이름과 경로는 보내지 않습니다."
            ),
            text(&labels[0])
        );
        assert_eq!(
            grainy_i18n::tr("어떤 상황에서 무슨 일이 있었는지, 무엇이 있으면 좋을지 적어 주세요."),
            text(&texts["placeholder"])
        );
        assert_eq!(grainy_i18n::tr("보내기"), text(&texts["send"]));
        assert_eq!(grainy_i18n::tr("닫기"), text(&texts["close"]));

        let steps = run["steps"].as_array().unwrap();
        for (index, want) in steps.iter().enumerate() {
            let name = want["name"].as_str().unwrap();
            match name {
                "opened" => {}
                "empty" => {
                    model.text = "   ".into();
                    model.submit(&mut cx);
                }
                "sending" => {
                    let next = steps[index + 1]["name"].as_str().unwrap();
                    match next {
                        "failed" => model.text = "  첫 번째 의견  ".into(),
                        "second" => model.text = "둘째 의견".into(),
                        "failed x" => model.text = "x".into(),
                        "sent y" => model.text = "y".into(),
                        _ => {} // "sent": the same text again after the failure
                    }
                    model.submit(&mut cx);
                }
                _ => {
                    let started = Instant::now();
                    loop {
                        settings::pump(&mut cx);
                        if !model.sending() {
                            model.poll();
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(5));
                        assert!(started.elapsed() < Duration::from_secs(30));
                    }
                }
            }
            let calls = recorder.calls();
            let seen = references(&calls);
            let at = format!("{} / {name}", run["language"]);
            assert_eq!(fixed(&model.status, &seen, &order), text(&want["status"]), "{at}: status");
            assert_eq!(!model.sending(), want["send_enabled"].as_bool().unwrap(), "{at}: send enabled");
            assert_eq!(model.text, text(&want["message"]), "{at}: message");
            if let Some(expected) = want["calls"].as_array() {
                let got: Vec<Vec<String>> =
                    calls.iter().map(|(m, s, _)| vec![m.clone(), fixed(s, &seen, &order)]).collect();
                let want_calls: Vec<Vec<String>> = expected
                    .iter()
                    .map(|c| c.as_array().unwrap().iter().map(text_of).collect::<Vec<String>>())
                    .collect();
                assert_eq!(got, want_calls, "{at}: what would have been sent");
            }
            let stored: Vec<Value> = feedback::sent_references(&cx)
                .into_iter()
                .map(|(r, v, d)| {
                    json!({"ref": fixed(&r, &seen, &order), "version": v, "date": Regex::new(r"^\d{4}-\d\d-\d\d$").unwrap().is_match(&d)})
                })
                .collect();
            assert_eq!(Value::Array(stored), want["sent"], "{at}: the records");
        }
        assert!(recorder.calls().iter().all(|(_, _, version)| version == grainy_platform::BUILD_VERSION));
        // The settings file holds the records and nothing else the dialog made up.
        assert!(prefs_of(&folder).values.contains_key("feedback_sent"));
    }
    grainy_i18n::set_language(Language::Korean);
}

fn text_of(value: &Value) -> String {
    text(value)
}

#[test]
fn the_system_line_is_the_oracles() {
    let g = Case::open("app", "feedback_dialog").json().clone();
    for line in g["system_lines"].as_array().unwrap() {
        let facts = SystemFacts {
            system: if line["system"] == "mac" { System::MacOs } else { System::Windows },
            mac_version: "14.5".into(),
            windows_release: "11".into(),
            windows_version: "10.0.26200".into(),
            machine: "AMD64".into(),
            gpu: (!text(&line["adapter"]).is_empty())
                .then(|| Gpu { adapter: text(&line["adapter"]), enabled: line["enabled"].as_bool().unwrap() }),
        };
        // The oracle's machine value on macOS is its own; only the shape is shared there.
        let got = grainy_platform::feedback::system_info(&facts);
        if line["system"] == "mac" {
            assert!(got.starts_with("macOS 14.5 · "), "{got}");
        } else {
            assert_eq!(got, text(&line["line"]));
        }
    }
}

#[test]
fn nothing_is_sent_by_default() {
    let _turn = turn();
    for language in ["ko", "en"] {
        set_language(language);
        let (mut cx, folder) = with_data(&format!("feedback-default-{language}"));
        let mut model = FeedbackModel::new(&cx);
        model.text = "hello".into();
        model.submit(&mut cx);
        drain(&mut cx);
        model.poll();
        let reason = grainy_app::startup::unavailable();
        assert!(model.status.contains(&reason), "{}", model.status);
        assert!(!prefs_of(&folder).values.contains_key("feedback_sent"), "a failed send leaves no record");
        assert!(!model.sending());
    }
    grainy_i18n::set_language(Language::Korean);
}

// ---- updates ----------------------------------------------------------------------------------------------------

fn release() -> Release {
    Release {
        version: "9.9.9".into(),
        name: "Grainy-9.9.9-windows-x64.zip".into(),
        url: "https://example.invalid/x.zip".into(),
        size: 1234,
        sha256: "ab".repeat(32),
        notes: String::new(),
    }
}

fn pref_bool(value: Option<&Pref>) -> Value {
    match value {
        Some(Pref::Bool(b)) => json!(b),
        _ => Value::Null,
    }
}

#[test]
fn the_update_questions_and_the_daily_check_follow_the_oracle() {
    let _turn = turn();
    let g = Case::open("app", "feedback_update").json().clone();
    for want in g["updates"].as_array().unwrap() {
        let name = want["name"].as_str().unwrap();
        let language = want["language"].as_str().unwrap();
        set_language(language);
        let at = format!("{name} / {language}");
        let (mut cx, folder) =
            with_data(&format!("feedback-update-{}-{language}", name.replace(' ', "-").replace(',', "")));
        let mut prefs = prefs_of(&folder);
        let seconds = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
        match name {
            "due never checked" | "crash checks at once" => prefs.save("update_check", Pref::Bool(true)).unwrap(),
            "due long ago" => {
                prefs.save("update_check", Pref::Bool(true)).unwrap();
                prefs.save("update_checked", Pref::Float(seconds - g["interval"].as_f64().unwrap() - 5.0)).unwrap();
            }
            "not due" => {
                prefs.save("update_check", Pref::Bool(true)).unwrap();
                prefs.save("update_checked", Pref::Float(seconds - 60.0)).unwrap();
            }
            "off" => prefs.save("update_check", Pref::Bool(false)).unwrap(),
            _ => {}
        }
        let fetched = Arc::new(AtomicUsize::new(0));
        let counter = fetched.clone();
        {
            let state = cx.state::<UpdateState>();
            state.packaged = Some(!name.contains("not packaged"));
            state.feed = Some("https://example.invalid/feed".into());
            state.opener = Some(Arc::new(move |_url: &str| {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(std::io::Cursor::new(b"{}".to_vec())) as Box<dyn Read>)
            }));
        }
        cx.state::<UpdateEnv>().supported = Some(true);
        cx.state::<UpdateEnv>().executable = Some(folder.join("app").join("Grainy.exe"));
        let before = cx.status().to_owned();
        let crash = name.starts_with("crash");
        if crash {
            update::after_crash(&mut cx);
        } else {
            update::startup(&mut cx);
        }
        let boxes = want["boxes"].as_array().unwrap();
        if let Some(expected) = boxes.first() {
            let make = || if crash { update::crash_box() } else { update::first_run_box() };
            let mut shown = make();
            assert!(cx.dialog_open(shown.id), "{at}: the question is open");
            assert_eq!(shown.title, text(&expected["title"]), "{at}");
            assert_eq!(shown.text, text(&expected["text"]), "{at}");
            assert_eq!(shown.informative, text(&expected["informative"]), "{at}");
            let names: Vec<&str> = shown.buttons.iter().map(|(n, _)| n.as_str()).collect();
            let want_names: Vec<String> = expected["buttons"].as_array().unwrap().iter().map(text_of).collect();
            assert_eq!(names, want_names, "{at}: buttons");
            let index = want["picked"].as_u64().unwrap_or_else(|| panic!("{at}: a button is pressed")) as usize;
            let action = shown.buttons[index].1.take();
            cx.close_dialog(shown.id);
            if let Some(action) = action {
                action(&mut cx);
            }
        } else {
            assert!(!cx.dialog_open("update_first_run") && !cx.dialog_open("update_crash"), "{at}: no question");
        }
        drain(&mut cx);
        let stored = prefs_of(&folder);
        assert_eq!(fetched.load(Ordering::SeqCst) as u64, want["fetch_calls"].as_u64().unwrap(), "{at}: feed reads");
        assert_eq!(pref_bool(stored.values.get("update_check")), want["update_check"], "{at}: update_check");
        assert_eq!(
            stored.values.contains_key("update_checked"),
            want["update_checked_saved"].as_bool().unwrap(),
            "{at}: update_checked"
        );
        assert_eq!(cx.dialog_open("update"), !want["infos"].as_array().unwrap().is_empty(), "{at}: the 'latest' box");
        let status = text(&want["status"]);
        if status.is_empty() {
            assert_eq!(cx.status(), before, "{at}: status");
        } else {
            assert_eq!(cx.status(), status, "{at}: status");
        }
    }
    grainy_i18n::set_language(Language::Korean);
}

struct FakeBackend {
    mode: String,
    launched: Mutex<Vec<(String, String, Vec<String>)>>,
}

impl Backend for FakeBackend {
    fn download_and_stage(
        &self,
        _release: &Release,
        work: &Path,
        progress: &mut dyn FnMut(u64, u64),
        _cancel: &AtomicBool,
    ) -> Result<Option<PathBuf>, String> {
        match self.mode.as_str() {
            "fail" => Err("boom".into()),
            "cancel" => Ok(None),
            _ => {
                progress(10, 10);
                Ok(Some(work.join("new")))
            }
        }
    }

    fn launch(&self, staged: &Path, app: &Path, arguments: &[String]) -> Result<(), String> {
        let name = |p: &Path| p.file_name().unwrap().to_string_lossy().into_owned();
        self.launched.lock().unwrap().push((name(staged), name(app), arguments.to_vec()));
        Ok(())
    }
}

#[test]
fn the_install_follows_the_oracle() {
    let _turn = turn();
    let g = Case::open("app", "feedback_update").json().clone();
    for want in g["installs"].as_array().unwrap() {
        let mode = want["name"].as_str().unwrap();
        let language = want["language"].as_str().unwrap();
        set_language(language);
        let at = format!("{mode} / {language}");
        let (mut cx, folder) = with_data(&format!("feedback-install-{mode}-{language}"));
        let backend = Arc::new(FakeBackend { mode: mode.into(), launched: Mutex::new(Vec::new()) });
        {
            let env = cx.state::<UpdateEnv>();
            env.backend = Some(backend.clone());
            env.executable = Some(folder.join("app").join("Grainy.exe"));
            env.arguments = Some(vec!["--probe".into()]);
            env.supported = Some(true);
        }
        let release_flag = Arc::new(AtomicBool::new(false));
        if mode == "busy" {
            let flag = release_flag.clone();
            cx.jobs.spawn(
                "export",
                "",
                move |_| {
                    while !flag.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Ok(())
                },
                |_: &mut AppContext, _: Result<(), String>| {},
            );
        }
        update::install(&mut cx, release());
        release_flag.store(true, Ordering::SeqCst);
        drain(&mut cx);
        let outcome = cx.state::<UpdateEnv>().outcome.clone();
        let infos = want["infos"].as_array().unwrap();
        let warnings = want["warnings"].as_array().unwrap();
        match mode {
            "busy" => {
                assert_eq!(outcome, Some(Outcome::Refused), "{at}");
                assert_eq!(update::busy_box().text, text(&infos[0]["text"]), "{at}");
                assert_eq!(update::busy_box().title, text(&infos[0]["title"]), "{at}");
                assert!(cx.dialog_open("update_busy") && !cx.dialog_open("workflow_progress"), "{at}");
            }
            "fail" => {
                assert_eq!(outcome, Some(Outcome::Failed("boom".into())), "{at}");
                assert_eq!(update::failed_box("boom").text, text(&warnings[0]["text"]), "{at}");
                assert_eq!(update::failed_box("boom").title, text(&warnings[0]["title"]), "{at}");
                assert!(cx.dialog_open("update_failed"), "{at}");
            }
            "cancel" => {
                assert_eq!(outcome, Some(Outcome::Cancelled), "{at}");
                assert!(!cx.dialog_open("update_failed"), "{at}");
            }
            _ => {
                assert_eq!(outcome, Some(Outcome::Handover), "{at}");
                let launched = backend.launched.lock().unwrap().clone();
                let expected: Vec<(String, String, Vec<String>)> = want["launched"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|l| {
                        (
                            text(&l["staged"]),
                            text(&l["app"]),
                            l["args"].as_array().unwrap().iter().map(text_of).collect(),
                        )
                    })
                    .collect();
                assert_eq!(launched, expected, "{at}");
            }
        }
        assert_eq!(want["result"].as_bool().unwrap(), mode == "ok", "{at}: the oracle's return value");
        assert_eq!(want["closed"].as_bool().unwrap(), outcome == Some(Outcome::Handover), "{at}: the window closes");
    }
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_default_backend_prepares_nothing() {
    let _turn = turn();
    set_language("en");
    let (mut cx, folder) = with_data("feedback-install-default");
    {
        let env = cx.state::<UpdateEnv>();
        env.executable = Some(folder.join("app").join("Grainy.exe"));
        env.supported = Some(true);
    }
    update::install(&mut cx, release());
    drain(&mut cx);
    let reason = grainy_app::startup::tr("이 빌드에서는 업데이트를 내려받을 수 없습니다.");
    assert_eq!(cx.state::<UpdateEnv>().outcome, Some(Outcome::Failed(reason.clone())));
    assert_eq!(reason, "Updates cannot be downloaded in this build.");
    assert!(!folder.join("app").exists(), "nothing was written next to the program");
    // The feed of the program fails the same way: a check reports it.
    let state = cx.state::<UpdateState>();
    state.opener = Some(startup::unavailable_opener());
    state.feed = Some("https://example.invalid/feed".into());
    settings::check_for_update(&mut cx, true);
    drain(&mut cx);
    assert!(cx.dialog_open("update"), "the failed check is reported");
    grainy_i18n::set_language(Language::Korean);
}

// ---- safe start -------------------------------------------------------------------------------------------------

fn marker(folder: &Path) -> Value {
    match std::fs::read_to_string(folder.join(grainy_platform::safe_start::NAME)) {
        Err(_) => Value::Null,
        Ok(text) => {
            let data: Value = serde_json::from_str(&text).unwrap();
            json!({
                "version": data["version"],
                "pid_is_own": data["pid"].as_u64() == Some(u64::from(std::process::id())),
                "started_is_number": data["started"].is_number(),
            })
        }
    }
}

#[test]
fn the_crash_marker_follows_the_oracle() {
    let g = Case::open("app", "feedback_safe_start").json().clone();
    assert_eq!(g["name"], grainy_platform::safe_start::NAME);
    let (mut cx, folder) = with_data("feedback-safe-start");
    let steps = g["steps"].as_array().unwrap();
    let write = |version: &str| {
        std::fs::write(
            folder.join(grainy_platform::safe_start::NAME),
            json!({"pid": 99_999_999, "version": version, "started": 1.0}).to_string(),
        )
        .unwrap();
    };
    for (index, want) in steps.iter().enumerate() {
        let at = text(&want["name"]);
        let got = match index {
            0 | 1 | 3 => json!(startup::begin(&mut cx)),
            2 => {
                startup::shutdown(&mut cx);
                Value::Null
            }
            4 => {
                write("0.0.1");
                json!(startup::begin(&mut cx))
            }
            5 => {
                write("0.0.2");
                startup::plan(&mut cx, &folder, false);
                startup::shutdown(&mut cx);
                Value::Null
            }
            _ => {
                std::fs::write(folder.join(grainy_platform::safe_start::NAME), "not json").unwrap();
                json!(startup::begin(&mut cx))
            }
        };
        // The oracle returns the previous run's version where a run did not close; here that is `true`.
        let expected = match &want["result"] {
            Value::String(_) => json!(true),
            other => other.clone(),
        };
        assert_eq!(got, expected, "{at}");
        assert_eq!(marker(&folder), want["marker"], "{at}: the marker file");
    }
    // Without a data folder (a session) nothing is written and nothing is crashed.
    let mut session = context();
    assert!(!startup::begin(&mut session));
    assert!(!startup::safe_mode(&session));
}

fn golden_library(name: &str) -> (PathBuf, Catalog) {
    let meta = Case::open("app", "library").json().clone();
    let dir = common::temp_dir(name);
    std::fs::copy(grainy_testkit::root().join("golden/app/library/start.sqlite"), dir.join("catalog.sqlite"))
        .expect("copy the library");
    let stamp = meta["stamp"].as_str().unwrap().to_owned();
    let token = meta["token"].as_str().unwrap().to_owned();
    let catalog = Catalog::with_sources(&dir, Sources::new(move || stamp.clone(), move || token.clone())).unwrap();
    (dir, catalog)
}

/// `restore_folder_selection`: in safe mode no photo opens by itself (`restore_photo_id=None`,
/// `refresh_lists(ensure_current=False)`), otherwise the saved one, or the first.
#[test]
fn safe_mode_opens_no_photo_by_itself() {
    let g = Case::open("app", "feedback_safe_start").json().clone();
    for want in g["restores"].as_array().unwrap() {
        let safe = want["safe"].as_bool().unwrap();
        let saved = want["saved"].as_u64();
        let at = format!("{want}");
        let (dir, catalog) = golden_library(&format!("feedback-restore-{safe}-{}", saved.is_some()));
        let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
        let ids: Vec<i64> = {
            let db = grainy_catalog::rusqlite::Connection::open(dir.join("catalog.sqlite")).unwrap();
            let mut statement = db.prepare("SELECT id FROM photos ORDER BY id").unwrap();
            statement.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect()
        };
        let chosen = saved.map(|_| ids[2] as u64);
        doc.save_preference(
            "folder_panel",
            &json!({"selected": null, "include_subfolders": true, "expanded": [], "current_photo": chosen}),
        )
        .unwrap();
        let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc));
        cx.set_data_folder(Some(dir.clone()));
        startup::plan(&mut cx, &dir, safe);
        grainy_app::session_state::poll(&mut cx);
        cx.doc.refresh().unwrap();
        grainy_app::session_state::poll(&mut cx);
        let current = Some(cx.doc.current_id()).filter(|id| *id != 0);
        let first = cx.doc.photos().first().map(|p| p.id);
        if want["restore_photo_id"].is_null() && !want["ensure_current"].as_bool().unwrap() {
            assert_eq!(current, None, "{at}: safe mode opens no photo");
        } else if want["restore_photo_id"].is_null() {
            assert_eq!(current, first, "{at}: the first photo opens");
            assert!(current.is_some(), "{at}");
        } else {
            assert_eq!(current, chosen, "{at}: the saved photo opens");
        }
    }
}

#[test]
fn the_steps_after_the_window_wait_for_dialogs() {
    let (mut cx, folder) = with_data("feedback-steps");
    startup::plan(&mut cx, &folder, true);
    assert_eq!(cx.state::<startup::Startup>().steps, VecDeque::from([Step::AfterCrash, Step::CheckFolders]));
    assert!(startup::safe_mode(&cx));
    cx.open_dialog(Box::new(MessageBox::information("probe", "t".into(), "x".into())));
    for _ in 0..10 {
        startup::frame(&mut cx);
    }
    assert_eq!(cx.state::<startup::Startup>().steps.len(), 2, "a question does not cover another");
    cx.close_dialog("probe");
    for _ in 0..10 {
        startup::frame(&mut cx);
    }
    assert!(cx.state::<startup::Startup>().steps.is_empty());
    assert_eq!(
        cx.status(),
        grainy_i18n::tr("지난번에 Grainy가 정상적으로 닫히지 않아 사진을 자동으로 열지 않았습니다.")
    );
    // A run that closed asks about updates after the window.
    startup::plan(&mut cx, &folder, false);
    assert_eq!(cx.state::<startup::Startup>().steps.front(), Some(&Step::UpdateStartup));
    assert!(!startup::safe_mode(&cx));
}

// ---- folder permission guide -----------------------------------------------------------------------------------

#[test]
fn the_folder_guide_says_what_the_oracle_says() {
    let _turn = turn();
    let g = Case::open("app", "feedback_file_access").json().clone();
    assert_eq!(g["url"], grainy_library::file_access::SETTINGS_URL);
    for want in g["guides"].as_array().unwrap() {
        set_language(want["language"].as_str().unwrap());
        let mac = want["mac"].as_bool().unwrap();
        let place = text(&want["place"]);
        let shown = &want["box"];
        let ours = startup::guide_box(&place, mac);
        let at = format!("{want}");
        assert_eq!(ours.title, text(&shown["title"]), "{at}");
        assert_eq!(ours.text, text(&shown["text"]), "{at}");
        assert_eq!(ours.informative, text(&shown["informative"]), "{at}");
        let names: Vec<String> = ours.buttons.iter().map(|(n, _)| n.clone()).collect();
        let want_names: Vec<String> = shown["buttons"].as_array().unwrap().iter().map(text_of).collect();
        assert_eq!(names, want_names, "{at}");
    }
    grainy_i18n::set_language(Language::Korean);
    // Once per run; with another dialog open it waits for that one.
    let mut cx = context();
    cx.open_dialog(Box::new(MessageBox::information("probe", "t".into(), "x".into())));
    assert!(!startup::guide(&mut cx, "D:\\Photos"));
    assert!(!cx.dialog_open("file_access"));
    cx.close_dialog("probe");
    cx.state::<startup::Startup>().begun = true;
    startup::frame(&mut cx);
    assert!(cx.dialog_open("file_access"), "shown when the other dialog closed");
    cx.close_dialog("file_access");
    assert!(!startup::guide(&mut cx, "D:\\Photos"), "once per run");
}

// ---- cloud backup: the functions ------------------------------------------------------------------------------------

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect()
}

fn names_of(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(text_of).collect()
}

/// A recycle bin that only records and deletes.
fn fake_bin(recycled: &Mutex<Vec<String>>) -> impl Fn(&Path) -> bool + '_ {
    move |path: &Path| {
        recycled.lock().unwrap().push(path.file_name().unwrap().to_string_lossy().into_owned());
        std::fs::remove_file(path).is_ok()
    }
}

#[test]
fn the_backup_functions_equal_the_oracles() {
    let _turn = turn();
    set_language("ko");
    let g = Case::open("app", "feedback_cloud_functions").json().clone();
    let c = &g["constants"];
    assert_eq!(cloud_backup::PREF, text(&c["pref"]));
    assert_eq!(cloud_backup::PREFIX, text(&c["prefix"]));
    assert_eq!(cloud_backup::FOLDER_NAME, text(&c["folder_name"]));
    assert_eq!(cloud_backup::KEEP as u64, c["keep"].as_u64().unwrap());
    assert_eq!(cloud_backup::INTERVAL.as_millis() as u64, c["interval_ms"].as_u64().unwrap());
    assert_eq!(cloud_backup::CLOSE_TIMEOUT.as_secs(), c["close_timeout"].as_u64().unwrap());

    let work = common::temp_dir("feedback-cloud-functions");
    let content = bytes(g["digest"]["hex"].as_str().unwrap());
    let a = work.join("a.sqlite");
    std::fs::write(&a, &content).unwrap();
    assert_eq!(cloud_backup::digest(&a).unwrap(), text(&g["digest"]["sha256"]));

    let other = bytes(g["uploads_other_hex"].as_str().unwrap());
    let target = work.join("target");
    let bin = Mutex::new(Vec::new());
    let when = |s: u32| chrono::NaiveDate::from_ymd_opt(2026, 3, 4).unwrap().and_hms_opt(5, 6, s).unwrap();
    let mut first_hash = String::new();
    for want in g["uploads"].as_array().unwrap() {
        let name = want["name"].as_str().unwrap();
        let (data, last, at) = match name {
            "first" => (content.clone(), None, when(7)),
            "unchanged" => (content.clone(), Some(first_hash.clone()), when(7)),
            "changed" => (other.clone(), Some(first_hash.clone()), when(8)),
            "same second" | "same second again" => (other.clone(), None, when(8)),
            _ => (b"this is not a database file".repeat(50), None, when(7)),
        };
        let staged = work.join(format!("{}.staged.sqlite", name.replace(' ', "-")));
        std::fs::write(&staged, &data).unwrap();
        let result = cloud_backup::upload(&staged, &target, last.as_deref(), cloud_backup::KEEP, at, &fake_bin(&bin));
        match &result {
            Ok((path, hash)) => {
                assert!(want["error"].is_null(), "{name}: the oracle fails");
                assert_eq!(
                    path.as_ref().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()),
                    want["target"].as_str().map(str::to_owned),
                    "{name}"
                );
                assert_eq!(hash, &text(&want["hash"]), "{name}");
                if let Some(path) = path {
                    assert_eq!(std::fs::read(path).unwrap() == data, want["copied"].as_bool().unwrap(), "{name}");
                }
                if name == "first" {
                    first_hash = hash.clone();
                }
            }
            Err(_) => assert!(!want["error"].is_null(), "{name}: only the oracle's bad file fails"),
        }
        assert_eq!(!staged.exists(), want["staged_removed"].as_bool().unwrap(), "{name}: the staged file goes");
        let parts = std::fs::read_dir(&target)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "part"))
            .count();
        assert_eq!(parts as u64, want["part_files"].as_u64().unwrap(), "{name}: no partial copy is left");
    }
    let mut listing: Vec<String> =
        std::fs::read_dir(&target).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    listing.sort();
    assert_eq!(listing, names_of(&g["folder_listing"]));

    for case in g["surplus"].as_array().unwrap() {
        let names = names_of(&case["names"]);
        assert_eq!(
            cloud_backup::surplus(&names, case["keep"].as_u64().unwrap() as usize),
            names_of(&case["out"]),
            "keep {}",
            case["keep"]
        );
    }

    let trim = &g["trim"];
    let folder = common::temp_dir("feedback-cloud-trim");
    for name in names_of(&trim["names"]) {
        std::fs::write(folder.join(name), b"x").unwrap();
    }
    std::fs::write(folder.join("other.txt"), b"x").unwrap();
    std::fs::create_dir(folder.join(format!("{}sub.sqlite", cloud_backup::PREFIX))).unwrap();
    let recycled = Mutex::new(Vec::new());
    cloud_backup::trim(&folder, trim["keep"].as_u64().unwrap() as usize, &fake_bin(&recycled));
    let mut got = recycled.lock().unwrap().clone();
    got.sort();
    assert_eq!(got, names_of(&trim["recycled"]));
    let mut left: Vec<String> =
        std::fs::read_dir(&folder).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, names_of(&trim["left"]), "a backup that is not recycled stays; nothing is deleted for good");
    // A recycle bin that fails leaves every backup in place.
    let before = std::fs::read_dir(&folder).unwrap().count();
    cloud_backup::trim(&folder, 0, &|_| false);
    assert_eq!(std::fs::read_dir(&folder).unwrap().count(), before);
}

// ---- cloud backup: the controller and the dialog -----------------------------------------------------------------------

/// An uploader that records its calls instead of copying.
struct FakeUploader {
    calls: Mutex<Vec<(PathBuf, Option<String>)>>,
    delay: Duration,
    hash: String,
}

impl Uploader for FakeUploader {
    fn upload(
        &self,
        staged: &Path,
        folder: &Path,
        last_hash: Option<&str>,
    ) -> Result<(Option<PathBuf>, String), String> {
        std::thread::sleep(self.delay);
        assert!(staged.is_file(), "the snapshot exists when it is handed over");
        self.calls.lock().unwrap().push((folder.to_path_buf(), last_hash.map(str::to_owned)));
        let _ = std::fs::remove_file(staged);
        Ok((Some(folder.join("x.sqlite")), self.hash.clone()))
    }
}

fn fake_uploader(delay: Duration, hash: &str) -> Arc<FakeUploader> {
    Arc::new(FakeUploader { calls: Mutex::new(Vec::new()), delay, hash: hash.into() })
}

#[test]
fn finishing_a_backup_says_what_the_oracle_says() {
    let _turn = turn();
    let g = Case::open("app", "feedback_cloud_backup").json().clone();
    for want in g["finish"].as_array().unwrap() {
        let language = want["language"].as_str().unwrap();
        set_language(language);
        let (mut cx, _) = with_library(&format!("feedback-cloud-finish-{language}"));
        cx.state::<CloudState>().now = Some("2026-03-04T05:06:07".into());
        let s = cloud_backup::settings(&cx);
        assert_eq!(
            (s.enabled, s.folder.as_str(), s.last.as_str(), s.hash.as_str(), s.error.as_str()),
            (false, "", "", "", "")
        );
        assert_eq!(want["defaults"], json!({"enabled": false, "error": "", "folder": "", "hash": "", "last": ""}));

        let notes = Arc::new(Mutex::new(Vec::<String>::new()));
        let notify = |notes: &Arc<Mutex<Vec<String>>>| -> cloud_backup::Notify {
            let notes = notes.clone();
            Box::new(move |_, message| notes.lock().unwrap().push(message))
        };
        cloud_backup::finish(&mut cx, Ok((None, "h1".into())), false, Some(notify(&notes)));
        let s = cloud_backup::settings(&cx);
        assert_eq!(*notes.lock().unwrap(), names_of(&want["skipped"]["notes"]));
        assert_eq!(
            (s.enabled, s.folder.clone(), s.hash.clone(), s.error.clone()),
            (false, String::new(), "h1".into(), String::new())
        );
        assert!(s.last.is_empty() && want["skipped"]["last_empty"].as_bool().unwrap());

        notes.lock().unwrap().clear();
        let path = PathBuf::from("X").join(format!("{}20260304-050607.sqlite", cloud_backup::PREFIX));
        cloud_backup::finish(&mut cx, Ok((Some(path), "h2".into())), true, Some(notify(&notes)));
        let s = cloud_backup::settings(&cx);
        let got: Vec<String> = notes.lock().unwrap().iter().map(|n| clean(n, &[])).collect();
        assert_eq!(got, names_of(&want["target"]["notes"]));
        assert_eq!((s.hash.as_str(), s.error.as_str()), ("h2", ""));
        assert_eq!(s.last, "2026-03-04T05:06:07");
        assert!(want["target"]["last_is_iso"].as_bool().unwrap());

        cx.set_status("");
        cloud_backup::finish(&mut cx, Err("boom".into()), true, None);
        assert_eq!(cx.status(), text(&want["error"]["status"]));
        assert_eq!(cloud_backup::settings(&cx).error, text(&want["error"]["settings_error"]));
        cx.set_status("");
        cloud_backup::finish(&mut cx, Ok((None, "h3".into())), true, None);
        assert_eq!(cx.status(), text(&want["manual_skipped_status"]));
        cx.set_status("");
        cloud_backup::finish(&mut cx, Ok((None, "h3".into())), false, None);
        assert_eq!(cx.status(), text(&want["auto_skipped_status"]), "an automatic backup is silent");
    }
    grainy_i18n::set_language(Language::Korean);
}

fn sqlite_files(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".sqlite"))
        .collect();
    names.sort();
    names
}

#[test]
fn starting_a_backup_follows_the_oracle() {
    let _turn = turn();
    set_language("ko");
    let g = Case::open("app", "feedback_cloud_backup").json().clone();
    let (mut cx, home) = with_library("feedback-cloud-start");
    let target = home.join("drive");
    let hex64 = Regex::new(r"^[0-9a-f]{64}$").unwrap();
    for want in g["start"].as_array().unwrap() {
        let name = want["name"].as_str().unwrap();
        let manual = want["manual"].as_bool().unwrap();
        let notes = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = notes.clone();
        let notify: cloud_backup::Notify = Box::new(move |_, message| sink.lock().unwrap().push(message));
        let started = match name {
            "no folder" => cloud_backup::start(&mut cx, manual, None),
            "automatic but off" => {
                cloud_backup::save(&mut cx, &[("folder", json!(target.to_string_lossy()))]);
                cloud_backup::start(&mut cx, manual, None)
            }
            "manual" => cloud_backup::start(&mut cx, manual, Some(notify)),
            "automatic" => {
                cloud_backup::save(&mut cx, &[("enabled", json!(true))]);
                cloud_backup::start(&mut cx, manual, Some(notify))
            }
            _ => {
                cx.state::<CloudState>().closing = true;
                let started = cloud_backup::start(&mut cx, manual, None);
                cx.state::<CloudState>().closing = false;
                started
            }
        };
        assert_eq!(started, want["started"].as_bool().unwrap(), "{name}");
        drain(&mut cx);
        if name == "manual" {
            let got: Vec<String> = notes.lock().unwrap().iter().map(|n| clean(n, &[&target])).collect();
            assert_eq!(got, names_of(&want["notes"]));
            let files: Vec<String> = sqlite_files(&target).iter().map(|f| clean(f, &[])).collect();
            assert_eq!(files, names_of(&want["files"]));
            let s = cloud_backup::settings(&cx);
            assert_eq!(s.error, text(&want["error"]));
            assert_eq!(hex64.is_match(&s.hash), want["hash_is_hex"].as_bool().unwrap());
            assert_eq!(!s.last.is_empty(), want["last_is_iso"].as_bool().unwrap());
            let staging = home.join("library").join("backups").join("cloud-staging");
            let left = std::fs::read_dir(&staging).map_or(0, |d| d.count());
            assert_eq!(left as u64, want["staging_left"].as_u64().unwrap(), "the staged snapshot is removed");
            // The copy is a whole catalogue in one file.
            let copy = grainy_catalog::rusqlite::Connection::open(target.join(&sqlite_files(&target)[0])).unwrap();
            let check: String = copy.query_row("PRAGMA quick_check", [], |r| r.get(0)).unwrap();
            assert_eq!(check, "ok");
            assert!(!target.join(format!("{}-wal", sqlite_files(&target)[0])).exists());
        }
    }
}

#[test]
fn the_schedule_and_the_final_backup() {
    let _turn = turn();
    set_language("ko");
    let (mut cx, home) = with_library("feedback-cloud-schedule");
    let target = home.join("drive");
    let uploader = fake_uploader(Duration::ZERO, "hash1");
    {
        let state = cx.state::<CloudState>();
        state.uploader = Some(uploader.clone());
        state.interval = Duration::from_millis(50);
        state.close_timeout = Duration::from_millis(300);
        state.now = Some("2026-03-04T05:06:07".into());
    }
    cloud_backup::save(&mut cx, &[("enabled", json!(true)), ("folder", json!(target.to_string_lossy()))]);
    let t0 = Instant::now();
    assert!(!cloud_backup::tick(&mut cx, t0), "the timer starts");
    assert!(!cloud_backup::tick(&mut cx, t0 + Duration::from_millis(10)));
    assert!(cloud_backup::tick(&mut cx, t0 + Duration::from_millis(60)), "every interval");
    drain(&mut cx);
    assert_eq!(
        uploader.calls.lock().unwrap().clone(),
        vec![(target.clone(), None)],
        "the first automatic backup has no hash yet"
    );
    assert_eq!(cloud_backup::settings(&cx).hash, "hash1");
    assert!(cloud_backup::tick(&mut cx, t0 + Duration::from_millis(200)));
    drain(&mut cx);
    assert_eq!(
        uploader.calls.lock().unwrap()[1].1.as_deref(),
        Some("hash1"),
        "an unchanged catalogue is recognised by its hash"
    );

    // The final backup on close.
    startup::shutdown(&mut cx);
    let calls = uploader.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 3, "one more on close");
    assert_eq!(calls[2], (target.clone(), Some("hash1".into())));
    assert_eq!(cloud_backup::settings(&cx).error, "");
    // Off: nothing on close.
    let (mut off, home2) = with_library("feedback-cloud-off");
    let quiet = fake_uploader(Duration::ZERO, "h");
    off.state::<CloudState>().uploader = Some(quiet.clone());
    cloud_backup::save(&mut off, &[("enabled", json!(false)), ("folder", json!(home2.join("d").to_string_lossy()))]);
    startup::shutdown(&mut off);
    assert!(quiet.calls.lock().unwrap().is_empty());
    // A backup that does not finish in time is reported and the window still closes.
    let (mut slow, home3) = with_library("feedback-cloud-slow");
    // Long enough that a loaded machine cannot finish it before the close gives up (100 ms).
    let late = fake_uploader(Duration::from_secs(6), "h");
    {
        let state = slow.state::<CloudState>();
        state.uploader = Some(late);
        state.close_timeout = Duration::from_millis(100);
    }
    cloud_backup::save(&mut slow, &[("enabled", json!(true)), ("folder", json!(home3.join("d").to_string_lossy()))]);
    let started = Instant::now();
    startup::shutdown(&mut slow);
    assert!(started.elapsed() < Duration::from_secs(4), "the close does not wait for ever");
    assert_eq!(cloud_backup::settings(&slow).error, grainy_i18n::tr("종료할 때 백업이 시간 안에 끝나지 않았습니다."));
    // No library, no timer.
    let mut session = context();
    assert!(!cloud_backup::tick(&mut session, Instant::now()));
}

#[test]
fn the_backup_window_follows_the_oracle() {
    let _turn = turn();
    let g = Case::open("app", "feedback_cloud_backup").json().clone();
    let hex = Regex::new(r"^[0-9a-f]{64}$").unwrap();
    for want in g["dialogs"].as_array().unwrap() {
        let name = want["name"].as_str().unwrap();
        let language = want["language"].as_str().unwrap();
        set_language(language);
        let at = format!("{name} / {language}");
        let (mut cx, home) = with_library(&format!("feedback-cloud-dialog-{}-{language}", name.replace(' ', "-")));
        let drive = want["drive"].as_bool().unwrap();
        cx.state::<CloudState>().drive = Some(if drive { Some(home.join("My Drive")) } else { None });
        if drive {
            std::fs::create_dir_all(home.join("My Drive")).unwrap();
        }
        if name == "saved" {
            cloud_backup::save(
                &mut cx,
                &[
                    ("enabled", json!(true)),
                    ("folder", json!(home.join("out").to_string_lossy())),
                    ("last", json!("2026-01-02T03:04:05")),
                    ("error", json!("boom")),
                ],
            );
        }
        let mut model = CloudBackupModel::new(&mut cx);
        let opened = &want["opened"];
        assert_eq!(model.enabled, opened["checked"][0].as_bool().unwrap(), "{at}");
        assert_eq!(clean(&model.folder, &[&home]), text(&opened["line"][0]), "{at}: the folder field");
        let labels = opened["labels"].as_array().unwrap();
        let status_prefix = grainy_i18n::tr("마지막 백업: {0}").split("{0}").next().unwrap_or("").to_owned();
        let status_label = labels.iter().map(text_of).find(|l| l.starts_with(&status_prefix)).unwrap_or_default();
        assert_eq!(clean(&model.status, &[&home]), clean(&status_label, &[&home]), "{at}: the status line");
        assert_eq!(CloudBackupModel::drive_missing(&mut cx), !drive, "{at}: the missing-drive line");
        let missing_line = grainy_i18n::tr(
            "Google Drive 데스크톱 앱을 찾지 못했습니다. 설치하고 로그인하면 내 드라이브 폴더를 고를 수 있습니다.",
        );
        assert_eq!(labels.iter().any(|l| text(l) == missing_line), !drive, "{at}");

        let out = home.join("out");
        match name {
            "enable and accept" => {
                model.enabled = true;
                model.folder = out.to_string_lossy().into_owned();
                model.accept(&mut cx);
            }
            "enable without a folder" => {
                model.enabled = true;
                model.folder = "  ".into();
                model.accept(&mut cx);
            }
            "now without a folder" => {
                model.folder.clear();
                model.run_now(&mut cx);
                let expected = want["final"]["status"].as_array().unwrap().iter().map(text_of).collect::<Vec<_>>();
                assert!(expected.contains(&model.status), "{at}: {}", model.status);
            }
            "now with a folder" => {
                model.folder = out.to_string_lossy().into_owned();
                model.run_now(&mut cx);
                assert!(model.running, "{at}: 지금 백업 is off while it runs");
                assert_eq!(model.status, grainy_i18n::tr("백업하는 중…"));
                drain(&mut cx);
                model.poll(&mut cx);
                assert!(!model.running, "{at}");
                let expected: Vec<String> = want["final"]["status"].as_array().unwrap().iter().map(text_of).collect();
                assert!(expected.contains(&clean(&model.status, &[&home])), "{at}: {} / {expected:?}", model.status);
            }
            _ => {}
        }
        drain(&mut cx);
        let s = cloud_backup::settings(&cx);
        let w = &want["settings"];
        assert_eq!(s.enabled, w["enabled"].as_bool().unwrap(), "{at}: enabled");
        assert_eq!(clean(&s.folder, &[&home]), text(&w["folder"]), "{at}: folder");
        assert_eq!(clean(&s.error, &[&home]), text(&w["error"]), "{at}: error");
        assert_eq!(!s.last.is_empty(), w["last_is_iso"].as_bool().unwrap(), "{at}: last");
        assert_eq!(hex.is_match(&s.hash), w["hash_is_hex"].as_bool().unwrap(), "{at}: hash");
        let files: Vec<String> = sqlite_files(&out).iter().map(|f| clean(f, &[])).collect();
        assert_eq!(files, names_of(&want["files"]), "{at}: files");
    }
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_backup_command_needs_a_library() {
    let session = context();
    assert!(!grainy_app::commands::enabled(&session, "file.cloud_backup"));
    assert!(grainy_app::commands::enabled(&session, "feedback.open"));
    let (cx, _) = with_library("feedback-cloud-command");
    assert!(grainy_app::commands::enabled(&cx, "file.cloud_backup"));
    let labels = ["구글 드라이브 백업…", "피드백 보내기"];
    for (id, label) in ["file.cloud_backup", "feedback.open"].iter().zip(labels) {
        assert_eq!(grainy_app::commands::find(id).unwrap().label, label);
    }
}

// ---- the window ----------------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn library_harness<'a>(dir: PathBuf) -> Harness<'a, GrainyApp> {
    builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.cx.set_document(Box::new(CatalogDocument::open(&dir).expect("a library")));
        app
    })
}

fn both_languages(mut each: impl FnMut(&str)) {
    let _turn = turn();
    for suffix in ["ko", "en"] {
        set_language(suffix);
        each(suffix);
    }
    grainy_i18n::set_language(Language::Korean);
}

use common::snapshot::{pin_status, save, settle};

#[test]
fn feedback_dialog() {
    both_languages(|suffix| {
        let dir = common::temp_dir("feedback-window");
        let mut harness = library_harness(dir.join("library"));
        settle(&mut harness);
        let app = harness.state_mut();
        let state = app.cx.state::<FeedbackState>();
        state.facts = Some(SystemFacts {
            system: System::Windows,
            mac_version: String::new(),
            windows_release: "11".into(),
            windows_version: "10.0.26200".into(),
            machine: "AMD64".into(),
            gpu: Some(Gpu { adapter: "Example GPU".into(), enabled: true }),
        });
        let mut dialog = FeedbackDialog::new(&app.cx);
        dialog.model.text = if suffix == "ko" {
            "사진을 열면 가끔 미리보기가 늦게 나옵니다.".into()
        } else {
            "Sometimes the preview is slow to appear when I open a photo.".into()
        };
        dialog.model.submit(&mut app.cx);
        app.cx.open_dialog(Box::new(dialog));
        settle(&mut harness);
        pin_status(harness.state_mut(), "");
        harness.run_steps(3);
        assert!(harness.state().cx.dialog_open("feedback"));
        save(&mut harness, &format!("feedback-dialog-{suffix}"));
    });
}

#[test]
fn feedback_cloud_backup_dialog() {
    both_languages(|suffix| {
        let dir = common::temp_dir("feedback-cloud-window");
        let mut harness = library_harness(dir.join("library"));
        settle(&mut harness);
        let app = harness.state_mut();
        app.cx.state::<CloudState>().drive = Some(None);
        // Fixed texts: the pictures never show a temporary folder.
        cloud_backup::save(
            &mut app.cx,
            &[
                ("enabled", json!(true)),
                ("folder", json!("G:\\My Drive\\Grainy 백업")),
                ("last", json!("2026-01-02T03:04:05")),
                ("error", json!("")),
            ],
        );
        let dialog = CloudBackupDialog::new(&mut app.cx);
        app.cx.open_dialog(Box::new(dialog));
        pin_status(harness.state_mut(), "");
        harness.run_steps(4);
        assert!(harness.state().cx.dialog_open("cloud_backup"));
        save(&mut harness, &format!("feedback-cloud-{suffix}"));
    });
}

#[test]
fn feedback_update_question_and_button() {
    both_languages(|suffix| {
        let dir = common::temp_dir("feedback-update-window");
        let mut harness = library_harness(dir.join("library"));
        settle(&mut harness);
        let app = harness.state_mut();
        app.cx.state::<UpdateState>().packaged = Some(true);
        app.cx.open_dialog(Box::new(update::first_run_box()));
        pin_status(harness.state_mut(), "");
        harness.run_steps(4);
        save(&mut harness, &format!("feedback-update-question-{suffix}"));
        harness.state_mut().cx.close_dialog("update_first_run");
        // A newer release found by the daily check: the header's button.
        settings::checked(&mut harness.state_mut().cx, false, Ok(Some(release())));
        pin_status(harness.state_mut(), "");
        harness.run_steps(4);
        save(&mut harness, &format!("feedback-update-button-{suffix}"));
        // The crash question and the folder-permission guide.
        harness.state_mut().cx.open_dialog(Box::new(update::crash_box()));
        harness.run_steps(4);
        save(&mut harness, &format!("feedback-crash-question-{suffix}"));
        harness.state_mut().cx.close_dialog("update_crash");
        harness.state_mut().cx.open_dialog(Box::new(startup::guide_box("D:\\Photos", false)));
        harness.run_steps(4);
        save(&mut harness, &format!("feedback-folder-guide-{suffix}"));
    });
}
