//! The real network of feedback and updates (HANDOFF2 A): the switch that turns it off, what a feedback post
//! carries, the Rust build's update rule on a feed shaped like today's, and the download, verification and
//! hand-over of `network::NetworkBackend` with a fake address reader and a fake launcher.
//!
//! **Nothing leaves the machine and no installer starts.** Every test switches the network off first
//! (`common::offline`); the release "downloads" come from memory and the launcher only records its arguments.

mod common;

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::settings::{self, UpdateState};
use grainy_app::dialogs::feedback::{FeedbackDialog, FeedbackModel, FeedbackState, Transport};
use grainy_app::dialogs::update::{self, Outcome, UpdateEnv};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::network::{self, HttpTransport, NetworkBackend};
use grainy_app::paths::Arguments;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::{AppContext, GrainyApp};
use grainy_i18n::Language;
use grainy_platform::feedback::{Gpu, Request, SendFailure, SystemFacts};
use grainy_platform::updater::{self, Layout, Release};
use grainy_platform::user_paths::System;
use sha2::{Digest, Sha256};

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

fn drain(cx: &mut AppContext) {
    let started = Instant::now();
    loop {
        settings::pump(cx);
        if cx.jobs.idle() {
            settings::pump(cx);
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
        assert!(started.elapsed() < Duration::from_secs(60), "jobs did not finish");
    }
}

fn wait_for_answer(cx: &mut AppContext, model: &mut FeedbackModel) {
    let started = Instant::now();
    loop {
        settings::pump(cx);
        if !model.sending() {
            model.poll();
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
        assert!(started.elapsed() < Duration::from_secs(30));
    }
}

fn facts() -> SystemFacts {
    SystemFacts {
        system: System::Windows,
        mac_version: String::new(),
        windows_release: "11".into(),
        windows_version: "10.0.26200".into(),
        machine: "AMD64".into(),
        gpu: Some(Gpu { adapter: "Example GPU".into(), enabled: true }),
    }
}

// ---- the switch ---------------------------------------------------------------------------------------------------

#[test]
fn the_switch_turns_every_network_use_off() {
    let _turn = turn();
    assert!(network::offline(), "the test harness switches the network off");
    let args = Arguments::parse(["--offline".into(), "photo.jpg".into()]).unwrap();
    assert!(args.offline);
    assert!(!Arguments::parse(Vec::new()).unwrap().offline);
    // The program's start installs nothing while the switch is on: the package 31 defaults stay.
    let mut cx = context();
    assert!(!network::install(&mut cx, true));
    assert!(!network::install(&mut cx, false), "GRAINY_OFFLINE / an earlier --offline still holds");
    assert!(cx.state::<FeedbackState>().transport.is_none());
    assert!(cx.state::<UpdateEnv>().backend.is_none());
    assert!(cx.state::<UpdateState>().opener.is_none());
    // The real implementations, built by hand, open nothing either.
    let reason = grainy_app::startup::unavailable();
    let request = Request { url: "https://example.invalid/".into(), headers: Vec::new(), body: Vec::new() };
    assert_eq!(HttpTransport.post(&request, 1), Err(SendFailure::Other(reason.clone())));
    let opened = network::http_opener()("https://example.invalid/feed").err().expect("no answer");
    assert_eq!(opened.to_string(), reason);
}

#[test]
fn the_feedback_window_on_the_real_transport_sends_nothing_while_offline() {
    let _turn = turn();
    set_language("en");
    let (mut cx, folder) = with_data("network-feedback-offline");
    cx.state::<FeedbackState>().transport = Some(Arc::new(HttpTransport));
    cx.state::<FeedbackState>().facts = Some(facts());
    let mut model = FeedbackModel::new(&cx);
    model.text = "hello".into();
    model.submit(&mut cx);
    wait_for_answer(&mut cx, &mut model);
    let reason = grainy_app::startup::unavailable();
    assert_eq!(
        model.status,
        grainy_app::text::tr_with("보내지 못했습니다 ({0}). 인터넷 연결을 확인하고 다시 시도해 주세요.", &[&reason]),
        "the oracle's failure line with the switch's reason"
    );
    let prefs = grainy_platform::preferences::Preferences::new(folder.join("settings.json"));
    assert!(!prefs.values.contains_key("feedback_sent"));
    grainy_i18n::set_language(Language::Korean);
}

// ---- what a feedback post carries ----------------------------------------------------------------------------------

struct Capture(Mutex<Vec<(Request, u64)>>);

impl Transport for Capture {
    fn post(&self, request: &Request, timeout: u64) -> Result<u16, SendFailure> {
        self.0.lock().unwrap().push((request.clone(), timeout));
        Ok(200)
    }
}

fn form(body: &[u8]) -> BTreeMap<String, String> {
    let decode = |text: &str| -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'+' => out.push(b' '),
                b'%' => {
                    out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                    i += 2;
                }
                b => out.push(b),
            }
            i += 1;
        }
        String::from_utf8(out).unwrap()
    };
    String::from_utf8(body.to_vec())
        .unwrap()
        .split('&')
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap();
            (decode(k), decode(v))
        })
        .collect()
}

#[test]
fn a_feedback_post_carries_the_oracles_fields_and_the_rust_version() {
    let _turn = turn();
    set_language("ko");
    let (mut cx, _folder) = with_data("network-feedback-fields");
    let capture = Arc::new(Capture(Mutex::new(Vec::new())));
    cx.state::<FeedbackState>().transport = Some(capture.clone());
    cx.state::<FeedbackState>().facts = Some(facts());
    cx.state::<FeedbackState>().today = Some("2026-01-02".into());
    let mut model = FeedbackModel::new(&cx);
    assert!(capture.0.lock().unwrap().is_empty(), "opening the window sends nothing");
    model.text = "  미리보기가 늦어요  ".into();
    model.submit(&mut cx);
    wait_for_answer(&mut cx, &mut model);
    let calls = capture.0.lock().unwrap().clone();
    assert_eq!(calls.len(), 1, "one post per press of 보내기");
    let (request, timeout) = &calls[0];
    assert_eq!(*timeout, 15, "feedback.TIMEOUT");
    assert_eq!(
        request.url,
        format!("https://docs.google.com/forms/d/e/{}/formResponse", grainy_platform::feedback::FORM_ID)
    );
    assert_eq!(
        request.headers,
        vec![
            ("User-Agent".to_string(), "Grainy/rs-0.1.0".to_string()),
            ("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()),
        ]
    );
    let fields = form(&request.body);
    let keys: Vec<&str> = fields.keys().map(String::as_str).collect();
    assert_eq!(keys, ["entry.1045781291", "entry.1065046570", "entry.839337160"], "exactly the oracle's three fields");
    assert_eq!(fields["entry.1045781291"], "rs-0.1.0", "the version field names the Rust build");
    assert_eq!(fields["entry.1065046570"], "미리보기가 늦어요");
    let system = &fields["entry.839337160"];
    assert!(system.starts_with("Windows 11 (10.0.26200) · AMD64 · GPU Example GPU · #"), "{system}");
    assert!(model.note().contains("Grainy rs-0.1.0"), "the window says what is sent: {}", model.note());
    grainy_i18n::set_language(Language::Korean);
}

// ---- updates --------------------------------------------------------------------------------------------------------

/// A release feed shaped like the Python Grainy's feed on 2026-11-19 (only `Grainy-` files): even if such a feed
/// were read (an override), nothing in it qualifies.
fn todays_feed() -> String {
    let asset = |name: &str, size: u64, hex: char| {
        format!(
            r#"{{"name": "{name}", "size": {size}, "digest": "sha256:{}", "browser_download_url": "{}v0.6.8/{name}"}}"#,
            hex.to_string().repeat(64),
            updater::ORACLE_DOWNLOADS
        )
    };
    format!(
        r#"{{"tag_name": "v0.6.8", "draft": false, "prerelease": false, "body": "notes", "assets": [{}, {}, {}]}}"#,
        asset("Grainy-0.6.8-macos-arm64.dmg", 105_809_575, 'd'),
        asset("Grainy-0.6.8-macos-arm64.zip", 92_349_723, 'c'),
        asset("Grainy-0.6.8-windows-x64.zip", 129_357_785, '0'),
    )
}

/// Without an override the check reads the Rust build's own feed, never the Python Grainy's.
#[test]
fn a_check_reads_the_rust_builds_own_feed() {
    let _turn = turn();
    set_language("ko");
    let (mut cx, _folder) = with_data("network-update-feed");
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    let state = cx.state::<UpdateState>();
    state.opener = Some(Arc::new(move |url: &str| {
        seen.lock().unwrap().push(url.to_string());
        Err(std::io::Error::other("no network in tests"))
    }));
    // `GRAINY_UPDATE_FEED` is not set by the tests; a set one would replace the feed.
    if std::env::var("GRAINY_UPDATE_FEED").is_ok_and(|v| !v.is_empty()) {
        return;
    }
    settings::check_for_update(&mut cx, true);
    drain(&mut cx);
    assert_eq!(*asked.lock().unwrap(), vec![updater::RS_FEED.to_string()]);
    assert_eq!(updater::RS_FEED, "https://api.github.com/repos/pastorius9/grainy-rs/releases/latest");
    assert!(!updater::RS_FEED.contains("/grainy/"), "never the Python Grainy's repository");
    assert!(updater::RS_PAGE.starts_with("https://github.com/pastorius9/grainy-rs/"));
    assert!(updater::RS_DOWNLOADS.starts_with("https://github.com/pastorius9/grainy-rs/releases/download/"));
    assert!(cx.dialog_open("update"), "the failure box");
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn a_check_of_todays_feed_ends_up_to_date() {
    let _turn = turn();
    set_language("ko");
    let (mut cx, folder) = with_data("network-update-today");
    let feed = folder.join("feed.json");
    std::fs::write(&feed, todays_feed()).unwrap();
    let asked = Arc::new(AtomicUsize::new(0));
    let counter = asked.clone();
    let state = cx.state::<UpdateState>();
    state.feed = Some(feed.to_string_lossy().into_owned());
    state.opener = Some(Arc::new(move |_url: &str| {
        counter.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::other("no network in tests"))
    }));
    settings::check_for_update(&mut cx, true);
    drain(&mut cx);
    assert_eq!(asked.load(Ordering::SeqCst), 0, "a local feed is read from the file");
    assert!(cx.dialog_open("update"), "the manual check answers in a box");
    assert!(cx.state::<UpdateState>().release.is_none(), "no release qualifies");
    let prefs = grainy_platform::preferences::Preferences::new(folder.join("settings.json"));
    assert!(prefs.values.contains_key("update_checked"), "the check succeeded (a failure records nothing)");
    assert_eq!(settings::latest_box().text, "최신 버전을 쓰고 있습니다 (0.1.0).");
    grainy_i18n::set_language(Language::Korean);
}

/// A Rust release zip (`updater::RS_PACKAGE`): one folder `GrainyRS/` with the program and its files.
fn release_zip() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for name in updater::RS_ITEMS {
            zip.start_file(format!("GrainyRS/{name}"), options).unwrap();
            let text = if name == "GrainyRS.exe" { "not a program".to_string() } else { format!("{name} text") };
            zip.write_all(text.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }
    buffer.into_inner()
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

type Launched = Arc<Mutex<Vec<(PathBuf, PathBuf, Vec<String>)>>>;

/// A backend whose downloads come from `served` and whose launcher records what it would start.
fn fake_backend(served: Vec<u8>, opened: Arc<Mutex<Vec<String>>>, launched: Launched) -> NetworkBackend {
    NetworkBackend {
        open: Arc::new(move |url: &str| {
            opened.lock().unwrap().push(url.to_string());
            Ok(Box::new(std::io::Cursor::new(served.clone())) as Box<dyn Read>)
        }),
        launcher: Arc::new(move |staged: &Path, app: &Path, arguments: &[String]| {
            launched.lock().unwrap().push((staged.to_path_buf(), app.to_path_buf(), arguments.to_vec()));
            Ok(())
        }),
        layout: Layout::current(),
    }
}

fn rust_release(bytes: &[u8]) -> Release {
    let tail = updater::asset_pattern(&Layout::current()).tail().to_string();
    let name = format!("{}0.7.0{tail}", updater::RUST_MARKER);
    Release {
        version: "0.7.0".into(),
        url: format!("{}v0.7.0/{name}", updater::RS_DOWNLOADS),
        name,
        size: bytes.len() as u64,
        sha256: hex(bytes),
        notes: String::new(),
    }
}

struct Install {
    outcome: Option<Outcome>,
    opened: Vec<String>,
    launched: Vec<(PathBuf, PathBuf, Vec<String>)>,
    app: PathBuf,
}

fn run_install(name: &str, served: Vec<u8>, release: Release) -> Install {
    let (mut cx, folder) = with_data(name);
    let app = folder.join("app");
    std::fs::create_dir_all(&app).unwrap();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let launched: Launched = Arc::new(Mutex::new(Vec::new()));
    {
        let env = cx.state::<UpdateEnv>();
        env.backend = Some(Arc::new(fake_backend(served, opened.clone(), launched.clone())));
        env.executable = Some(app.join("Grainy.exe"));
        env.arguments = Some(vec!["--probe".into()]);
        env.supported = Some(true);
    }
    update::install(&mut cx, release);
    drain(&mut cx);
    let outcome = cx.state::<UpdateEnv>().outcome.clone();
    let opened = opened.lock().unwrap().clone();
    let launched = launched.lock().unwrap().clone();
    Install { outcome, opened, launched, app }
}

#[cfg(windows)]
#[test]
fn a_rust_release_is_downloaded_verified_unpacked_and_handed_over() {
    let _turn = turn();
    set_language("ko");
    let bytes = release_zip();
    let release = rust_release(&bytes);
    let got = run_install("network-install-ok", bytes, release.clone());
    assert_eq!(got.outcome, Some(Outcome::Handover));
    assert_eq!(got.opened, vec![release.url.clone()], "one download of the release's own address");
    let work = got.app.join(".update");
    assert_eq!(got.launched, vec![(work.join("new"), got.app.clone(), vec!["--probe".to_string()])]);
    assert_eq!(std::fs::read(work.join("new").join("GrainyRS.exe")).unwrap(), b"not a program");
    assert!(work.join("new").join("THIRD_PARTY_NOTICES.txt").is_file());
    assert!(work.join("download").join(&release.name).is_file(), "the verified zip is kept, as the oracle keeps it");
    grainy_i18n::set_language(Language::Korean);
}

#[cfg(windows)]
#[test]
fn a_foreign_rust_release_is_refused_before_download() {
    let _turn = turn();
    let bytes = release_zip();
    let mut release = rust_release(&bytes);
    release.url = format!("https://example.invalid/{}", release.name);
    let got = run_install("network-install-foreign", bytes, release);
    assert!(matches!(got.outcome, Some(Outcome::Failed(_))));
    assert!(got.opened.is_empty());
    assert!(got.launched.is_empty());
    assert!(!got.app.join(".update").exists());
}

#[cfg(windows)]
#[test]
fn a_download_that_differs_is_refused_and_nothing_starts() {
    let _turn = turn();
    set_language("ko");
    let bytes = release_zip();
    // The digest of other bytes: the oracle's ValueError text, nothing unverified left behind.
    let mut release = rust_release(&bytes);
    release.sha256 = hex(b"other bytes");
    let got = run_install("network-install-digest", bytes.clone(), release.clone());
    assert_eq!(got.outcome, Some(Outcome::Failed("내려받은 파일이 깃헙에 올라간 파일과 다릅니다.".into())));
    assert!(got.launched.is_empty(), "nothing starts");
    let download = got.app.join(".update").join("download");
    assert_eq!(std::fs::read_dir(&download).unwrap().count(), 0, "no file and no .part remains");
    // A size that differs fails the same way.
    let mut release = rust_release(&bytes);
    release.size += 1;
    let got = run_install("network-install-size", bytes, release);
    assert_eq!(got.outcome, Some(Outcome::Failed("내려받은 파일이 깃헙에 올라간 파일과 다릅니다.".into())));
    assert!(got.launched.is_empty());
    grainy_i18n::set_language(Language::Korean);
}

#[cfg(windows)]
#[test]
fn a_zip_in_the_python_layout_is_refused_and_nothing_starts() {
    let _turn = turn();
    set_language("ko");
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("Grainy/Grainy.exe", options).unwrap();
        zip.write_all(b"python").unwrap();
        zip.start_file("Grainy/_internal/x", options).unwrap();
        zip.write_all(b"x").unwrap();
        zip.finish().unwrap();
    }
    let bytes = buffer.into_inner();
    let got = run_install("network-install-layout", bytes.clone(), rust_release(&bytes));
    assert_eq!(got.outcome, Some(Outcome::Failed("업데이트 파일의 구조가 올바르지 않습니다.".into())));
    assert!(got.launched.is_empty(), "nothing starts");
    assert!(!got.app.join(".update").join("new").exists(), "nothing is staged");
    grainy_i18n::set_language(Language::Korean);
}

#[cfg(windows)]
#[test]
fn a_python_release_is_never_downloaded_or_started() {
    let _turn = turn();
    set_language("ko");
    let bytes = release_zip();
    let mut release = rust_release(&bytes);
    release.name = release.name.replacen(updater::RUST_MARKER, "Grainy-", 1);
    release.url = format!("https://example.invalid/{}", release.name);
    let got = run_install("network-install-python", bytes, release);
    assert_eq!(got.outcome, Some(Outcome::Failed("업데이트 파일의 구조가 올바르지 않습니다.".into())));
    assert!(got.opened.is_empty(), "the Python installer's address is never opened");
    assert!(got.launched.is_empty(), "nothing starts");
    assert!(!got.app.join(".update").exists(), "nothing is written");
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_finish_step_reads_its_command_line() {
    let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert_eq!(network::finish_arguments(&args(&["--library", "x"])), None);
    assert_eq!(network::run_finish(&args(&["photo.jpg"])), None, "an ordinary start is not the finish step");
    assert_eq!(
        network::finish_arguments(&args(&[
            "--finish-update",
            "C:\\App",
            "--wait-pid",
            "42",
            "--",
            "--memory",
            "a.jpg"
        ])),
        Some((PathBuf::from("C:\\App"), 42, args(&["--memory", "a.jpg"])))
    );
    assert_eq!(
        network::finish_arguments(&args(&["--finish-update", "C:\\App", "--wait-pid", "42"])),
        Some((PathBuf::from("C:\\App"), 42, Vec::new()))
    );
    assert_eq!(network::finish_arguments(&args(&["--finish-update", "C:\\App", "--wait-pid", "x"])), None);
}

// ---- the window ----------------------------------------------------------------------------------------------------

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
fn network_feedback_offline() {
    both_languages(|suffix| {
        let dir = common::temp_dir("network-feedback-window");
        let mut harness = library_harness(dir.join("library"));
        settle(&mut harness);
        let app = harness.state_mut();
        let state = app.cx.state::<FeedbackState>();
        state.facts = Some(facts());
        state.transport = Some(Arc::new(HttpTransport));
        let mut dialog = FeedbackDialog::new(&app.cx);
        dialog.model.text = if suffix == "ko" {
            "업데이트 확인이 잘 됩니다.".into()
        } else {
            "The update check works.".into()
        };
        dialog.model.submit(&mut app.cx);
        app.cx.open_dialog(Box::new(dialog));
        settle(&mut harness);
        pin_status(harness.state_mut(), "");
        harness.run_steps(3);
        assert!(harness.state().cx.dialog_open("feedback"));
        save(&mut harness, &format!("network-feedback-offline-{suffix}"));
    });
}

#[test]
fn network_update_up_to_date() {
    both_languages(|suffix| {
        let dir = common::temp_dir("network-update-window");
        let feed = dir.join("feed.json");
        std::fs::write(&feed, todays_feed()).unwrap();
        let mut harness = library_harness(dir.join("library"));
        settle(&mut harness);
        let app = harness.state_mut();
        app.cx.state::<UpdateState>().feed = Some(feed.to_string_lossy().into_owned());
        settings::check_for_update(&mut app.cx, true);
        settle(&mut harness);
        pin_status(harness.state_mut(), "");
        harness.run_steps(3);
        assert!(harness.state().cx.dialog_open("update"));
        assert!(harness.state_mut().cx.state::<UpdateState>().release.is_none());
        save(&mut harness, &format!("network-update-latest-{suffix}"));
    });
}
