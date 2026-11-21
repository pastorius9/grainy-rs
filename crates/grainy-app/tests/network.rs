//! The real network of feedback and updates (HANDOFF2 A): the switch that turns it off, what a feedback post
//! carries, the Rust build's update rule on a feed shaped like today's, and the download, verification and
//! hand-over of `network::NetworkBackend` with a fake address reader and a fake launcher: on Windows the whole
//! update of a 0.1.0 program folder to 0.2.0, on every system the macOS disk image with fake `hdiutil`/`ditto`.
//!
//! **Nothing leaves the machine and no installer starts.** Every test switches the network off first
//! (`common::offline`); the release "downloads" come from memory and the launcher only records its arguments.

mod common;

use std::collections::BTreeMap;
use std::io::Read;
// Writing zips and installing them are the Windows tests' (the update of a program folder).
#[cfg(windows)]
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::settings::{self, UpdateState};
use grainy_app::dialogs::feedback::{FeedbackDialog, FeedbackModel, FeedbackState, Transport};
use grainy_app::dialogs::update::UpdateEnv;
#[cfg(windows)]
use grainy_app::dialogs::update::{self, Outcome};
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
            ("User-Agent".to_string(), format!("Grainy/rs-{}", env!("CARGO_PKG_VERSION"))),
            ("Content-Type".to_string(), "application/x-www-form-urlencoded".to_string()),
        ]
    );
    let fields = form(&request.body);
    let keys: Vec<&str> = fields.keys().map(String::as_str).collect();
    assert_eq!(keys, ["entry.1045781291", "entry.1065046570", "entry.839337160"], "exactly the oracle's three fields");
    assert_eq!(
        fields["entry.1045781291"],
        concat!("rs-", env!("CARGO_PKG_VERSION")),
        "the version field names the Rust build"
    );
    assert_eq!(fields["entry.1065046570"], "미리보기가 늦어요");
    let system = &fields["entry.839337160"];
    assert!(system.starts_with("Windows 11 (10.0.26200) · AMD64 · GPU Example GPU · #"), "{system}");
    assert!(
        model.note().contains(concat!("Grainy rs-", env!("CARGO_PKG_VERSION"))),
        "the window says what is sent: {}",
        model.note()
    );
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
    assert_eq!(settings::latest_box().text, concat!("최신 버전을 쓰고 있습니다 (", env!("CARGO_PKG_VERSION"), ")."));
    grainy_i18n::set_language(Language::Korean);
}

#[cfg(windows)]
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
        disk_image: Arc::new(updater::SystemTools),
    }
}

#[cfg(windows)]
fn rust_release(bytes: &[u8]) -> Release {
    let tail = updater::rust_asset_pattern(&Layout::current()).tail().to_string();
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

#[cfg(windows)]
struct Install {
    outcome: Option<Outcome>,
    opened: Vec<String>,
    launched: Vec<(PathBuf, PathBuf, Vec<String>)>,
    app: PathBuf,
}

#[cfg(windows)]
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

// ---- 0.2.0: the update from 0.1.0 (Windows) and the disk image (macOS) ------------------------------------------

#[cfg(windows)]
/// The files of 0.1.0's program folder (its `updater::RS_ITEMS`: no `vcomp140.dll`).
const ITEMS_0_1_0: [&str; 5] = ["GrainyRS.exe", "grainy-cli.exe", "LICENSE", "THIRD_PARTY_NOTICES.txt", "README.txt"];

#[cfg(windows)]
fn content(name: &str, version: &str) -> Vec<u8> {
    format!("{name} of Grainy RS {version}").into_bytes()
}

#[cfg(windows)]
/// The 0.2.0 release zip: `GrainyRS/` with its six items.
fn zip_0_2_0() -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = zip::ZipWriter::new(&mut buffer);
        for name in updater::RS_ITEMS {
            zip.start_file(format!("GrainyRS/{name}"), zip::write::SimpleFileOptions::default()).unwrap();
            zip.write_all(&content(name, "0.2.0")).unwrap();
        }
        zip.finish().unwrap();
    }
    buffer.into_inner()
}

/// The feed of release v0.2.0: the Rust build's Windows zip and macOS disk image beside Python Grainy files.
fn feed_0_2_0(zip: &[u8], dmg: &[u8]) -> String {
    let asset = |name: &str, bytes: &[u8]| {
        format!(
            r#"{{"name": "{name}", "size": {}, "digest": "sha256:{}", "browser_download_url": "{}v0.2.0/{name}"}}"#,
            bytes.len(),
            hex(bytes),
            updater::RS_DOWNLOADS
        )
    };
    format!(
        r#"{{"tag_name": "v0.2.0", "draft": false, "prerelease": false, "body": "Grainy RS 0.2.0", "assets": [{}, {}, {}, {}]}}"#,
        asset("Grainy-0.6.8-windows-x64.zip", b"python"),
        asset("Grainy-0.6.8-macos-arm64.zip", b"python mac"),
        asset("GrainyRS-0.2.0-macos-universal.dmg", dmg),
        asset("GrainyRS-0.2.0-windows-x64.zip", zip),
    )
}

#[cfg(windows)]
/// Every file and folder of the program folder except the work folder `.update`, with its bytes.
fn files(app: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Option<Vec<u8>>>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if rel == ".update" {
                continue;
            }
            if path.is_dir() {
                out.insert(rel, None);
                walk(root, &path, out);
            } else {
                out.insert(rel, Some(std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(app, app, &mut out);
    out
}

/// The whole update of a 0.1.0 program folder to 0.2.0 with fakes only: the check reads the feed through a fake
/// reader, the release zip "downloads" from memory, its size and digest are checked, it is staged, the launcher
/// only records, and the finishing step (what the staged 0.2.0 program runs) is called directly.
#[cfg(windows)]
#[test]
fn an_update_from_0_1_0_to_0_2_0() {
    let _turn = turn();
    set_language("ko");
    let (mut cx, folder) = with_data("network-from-0-1-0");
    let app = folder.join("Grainy RS");
    for name in ITEMS_0_1_0 {
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join(name), content(name, "0.1.0")).unwrap();
    }
    std::fs::create_dir_all(app.join("presets")).unwrap();
    std::fs::write(app.join("presets").join("warm.json"), b"{\"exposure\": 0.3}").unwrap();
    std::fs::write(app.join("my notes.txt"), b"the person's own file").unwrap();
    let before = files(&app);

    // The check: one read of the Rust build's feed; the release is newer than 0.1.0 (this build, 0.2.1, does
    // not offer 0.2.0, so it compares with the version a 0.1.0 build has).
    let zip = zip_0_2_0();
    let feed = feed_0_2_0(&zip, b"a disk image");
    let asked = Mutex::new(Vec::new());
    let open = |url: &str| -> std::io::Result<Box<dyn Read>> {
        asked.lock().unwrap().push(url.to_string());
        Ok(Box::new(std::io::Cursor::new(feed.clone().into_bytes())))
    };
    let release = updater::latest_rust(&open, None, &Layout::current()).unwrap().expect("the 0.2.0 zip qualifies");
    assert_eq!(*asked.lock().unwrap(), [updater::RS_FEED]);
    assert_eq!(release.name, "GrainyRS-0.2.0-windows-x64.zip");
    assert_eq!((release.size, release.sha256.clone()), (zip.len() as u64, hex(&zip)));
    assert!(updater::newer(&release.version, "0.1.0"));
    assert!(!updater::newer(&release.version, grainy_platform::RS_VERSION), "0.2.0 does not offer itself");

    // 지금 업데이트: download, digest check, staging, hand-over.
    let opened = Arc::new(Mutex::new(Vec::new()));
    let launched: Launched = Arc::new(Mutex::new(Vec::new()));
    {
        let env = cx.state::<UpdateEnv>();
        env.backend = Some(Arc::new(fake_backend(zip.clone(), opened.clone(), launched.clone())));
        env.executable = Some(app.join("GrainyRS.exe"));
        env.arguments = Some(vec!["--memory".into()]);
        env.supported = Some(true);
    }
    update::install(&mut cx, release.clone());
    drain(&mut cx);
    assert_eq!(cx.state::<UpdateEnv>().outcome, Some(Outcome::Handover));
    assert_eq!(*opened.lock().unwrap(), vec![release.url.clone()], "one download of the release's own address");
    let work = app.join(".update");
    let staged = work.join("new");
    assert_eq!(*launched.lock().unwrap(), [(staged.clone(), app.clone(), vec!["--memory".to_string()])]);
    assert_eq!(std::fs::read(work.join("download").join(&release.name)).unwrap(), zip, "the verified zip");
    for name in updater::RS_ITEMS {
        assert_eq!(std::fs::read(staged.join(name)).unwrap(), content(name, "0.2.0"), "staged {name}");
    }
    assert_eq!(files(&app), before, "nothing of the program folder changed before the finishing step");
    // What the real launcher starts, and what the started program reads from it.
    let (program, command, _) = updater::launch_command_for(
        &staged,
        &app,
        &["--memory".into()],
        4242,
        &Layout::current(),
        &updater::RS_PACKAGE,
    );
    assert_eq!(program, staged.join("GrainyRS.exe"));
    assert_eq!(network::finish_arguments(&command), Some((app.clone(), 4242, vec!["--memory".to_string()])));

    // The finishing step of the staged 0.2.0 program (`network::run_finish` without the restart).
    let error = updater::finish_logged_for(&app, &staged, 0, &Layout::current(), &updater::RS_PACKAGE);
    assert_eq!(error, "");
    for name in updater::RS_ITEMS {
        assert_eq!(std::fs::read(app.join(name)).unwrap(), content(name, "0.2.0"), "new {name}");
    }
    let previous: Vec<PathBuf> = std::fs::read_dir(&work)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("previous-"))
        .collect();
    assert_eq!(previous.len(), 1);
    let mut kept: Vec<String> = std::fs::read_dir(&previous[0])
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    kept.sort();
    let mut old: Vec<String> = ITEMS_0_1_0.iter().map(|s| s.to_string()).collect();
    old.sort();
    assert_eq!(kept, old, "the five 0.1.0 items are kept, nothing else");
    for name in ITEMS_0_1_0 {
        assert_eq!(std::fs::read(previous[0].join(name)).unwrap(), content(name, "0.1.0"));
    }
    let after = files(&app);
    for (path, bytes) in &before {
        if !ITEMS_0_1_0.contains(&path.as_str()) {
            assert_eq!(after.get(path), Some(bytes), "{path} is the person's own and unchanged");
        }
    }
    let added: Vec<&String> = after.keys().filter(|k| !before.contains_key(*k)).collect();
    assert_eq!(added, ["vcomp140.dll"]);
    let log = std::fs::read_to_string(work.join("update.log")).unwrap();
    assert!(log.contains("new version copied: ['GrainyRS.exe', 'grainy-cli.exe', 'vcomp140.dll',"), "{log}");
    grainy_i18n::set_language(Language::Korean);
}

/// Fake `hdiutil` and `ditto` for a Mac layout on any system: attaching copies `volume` into the mount folder,
/// detaching empties it; the calls are recorded.
struct FakeImage {
    volume: PathBuf,
    calls: Mutex<Vec<String>>,
    attached: Mutex<Option<PathBuf>>,
}

impl updater::DiskImageTools for FakeImage {
    fn attach(&self, image: &Path, mount: &Path) -> std::io::Result<()> {
        self.calls.lock().unwrap().push(format!("attach {}", image.file_name().unwrap().to_string_lossy()));
        updater::copy_tree(&self.volume.join(updater::RS_BUNDLE), &mount.join(updater::RS_BUNDLE))?;
        *self.attached.lock().unwrap() = Some(mount.to_path_buf());
        Ok(())
    }

    fn detach(&self, mount: &Path, force: bool) -> std::io::Result<()> {
        self.calls.lock().unwrap().push(if force { "detach -force" } else { "detach" }.into());
        std::fs::remove_dir_all(mount.join(updater::RS_BUNDLE))?;
        *self.attached.lock().unwrap() = None;
        Ok(())
    }

    fn copy(&self, source: &Path, target: &Path) -> std::io::Result<()> {
        self.calls.lock().unwrap().push("copy".into());
        updater::copy_tree(source, target)
    }

    fn mounted(&self, mount: &Path) -> bool {
        self.attached.lock().unwrap().as_deref() == Some(mount)
    }
}

fn info_plist(identifier: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>CFBundleExecutable</key><string>GrainyRS</string><key>CFBundleIdentifier</key><string>{identifier}</string></dict></plist>\n"
    )
}

fn mac_backend(
    root: &Path,
    identifier: &str,
    served: Vec<u8>,
    opened: Arc<Mutex<Vec<String>>>,
) -> (NetworkBackend, Arc<FakeImage>) {
    let bundle = root.join("volume").join(updater::RS_BUNDLE);
    std::fs::create_dir_all(bundle.join("Contents").join("MacOS")).unwrap();
    std::fs::write(bundle.join(updater::RS_PROGRAM), b"GrainyRS 0.2.0").unwrap();
    std::fs::write(bundle.join("Contents").join("Info.plist"), info_plist(identifier)).unwrap();
    let tools =
        Arc::new(FakeImage { volume: root.join("volume"), calls: Mutex::new(Vec::new()), attached: Mutex::new(None) });
    let mut backend = fake_backend(served, opened, Arc::new(Mutex::new(Vec::new())));
    backend.layout = Layout {
        system: System::MacOs,
        machine: "arm64".into(),
        env: grainy_platform::Environment { system: System::MacOs, local_app_data: None, home: root.to_path_buf() },
    };
    backend.disk_image = tools.clone();
    (backend, tools)
}

fn dmg_release(bytes: &[u8]) -> Release {
    let name = "GrainyRS-0.2.0-macos-universal.dmg".to_string();
    Release {
        version: "0.2.0".into(),
        url: format!("{}v0.2.0/{name}", updater::RS_DOWNLOADS),
        name,
        size: bytes.len() as u64,
        sha256: hex(bytes),
        notes: String::new(),
    }
}

#[test]
fn a_mac_release_is_downloaded_verified_and_staged_from_its_disk_image() {
    use grainy_app::dialogs::update::Backend;
    let _turn = turn();
    set_language("ko");
    let root = common::temp_dir("network-mac-image");
    let image = b"a universal disk image".to_vec();
    let opened = Arc::new(Mutex::new(Vec::new()));
    let (backend, tools) = mac_backend(&root, updater::RS_BUNDLE_ID, image.clone(), opened.clone());
    // The Mac's check takes the disk image from the 0.2.0 feed.
    let feed = feed_0_2_0(b"zip", &image);
    let open =
        |_: &str| -> std::io::Result<Box<dyn Read>> { Ok(Box::new(std::io::Cursor::new(feed.clone().into_bytes()))) };
    let release = updater::latest_rust(&open, None, &backend.layout).unwrap().unwrap();
    assert_eq!(release, Release { notes: "Grainy RS 0.2.0".into(), ..dmg_release(&image) });
    let app = PathBuf::from("/Applications/Grainy RS.app");
    let work = updater::work_folder_for(&app, &backend.layout, &updater::RS_PACKAGE);
    assert_eq!(work, root.join("Library").join("Application Support").join("GrainyRS").join("Update"));
    let mut seen = Vec::new();
    let staged = backend
        .download_and_stage(&release, &work, &mut |done, total| seen.push((done, total)), &AtomicBool::new(false))
        .unwrap()
        .unwrap();
    assert_eq!(staged, work.join("new").join("Grainy RS.app"));
    assert_eq!(std::fs::read(staged.join(updater::RS_PROGRAM)).unwrap(), b"GrainyRS 0.2.0");
    assert_eq!(*opened.lock().unwrap(), vec![release.url.clone()]);
    assert_eq!(seen.last(), Some(&(image.len() as u64, image.len() as u64)));
    assert_eq!(std::fs::read(work.join("download").join(&release.name)).unwrap(), image, "the verified image");
    assert_eq!(*tools.calls.lock().unwrap(), ["attach GrainyRS-0.2.0-macos-universal.dmg", "copy", "detach"]);
    // The staged bundle's own program finishes the update, from the folder that holds the bundle.
    let (program, command, cwd) =
        updater::launch_command_for(&staged, &app, &[], 7, &backend.layout, &updater::RS_PACKAGE);
    assert_eq!((program, cwd), (staged.join("Contents/MacOS/GrainyRS"), work.join("new")));
    assert_eq!(network::finish_arguments(&command), Some((app, 7, Vec::new())));
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn a_mac_refuses_other_files_and_other_bundles() {
    use grainy_app::dialogs::update::Backend;
    let _turn = turn();
    set_language("ko");
    let bad = "업데이트 파일의 구조가 올바르지 않습니다.";
    let image = b"a universal disk image".to_vec();
    let stage = |name: &str, identifier: &str, release: Release| {
        let root = common::temp_dir(name);
        let opened = Arc::new(Mutex::new(Vec::new()));
        let (backend, tools) = mac_backend(&root, identifier, image.clone(), opened.clone());
        let work = root.join("Update");
        let result = backend.download_and_stage(&release, &work, &mut |_, _| {}, &AtomicBool::new(false));
        let calls = tools.calls.lock().unwrap().clone();
        let opened = opened.lock().unwrap().clone();
        (result, opened, calls, work)
    };
    // The Python Grainy's macOS zip, and the Rust build's Windows zip, on a Mac: nothing is downloaded.
    for name in ["Grainy-0.2.0-macos-arm64.zip", "GrainyRS-0.2.0-windows-x64.zip", "GrainyRS-0.2.0-macos-arm64.zip"] {
        let release = Release {
            name: name.into(),
            url: format!("{}v0.2.0/{name}", updater::RS_DOWNLOADS),
            ..dmg_release(&image)
        };
        let (result, opened, calls, work) = stage("network-mac-other-file", updater::RS_BUNDLE_ID, release);
        assert_eq!(result, Err(bad.to_string()), "{name}");
        assert!(opened.is_empty() && calls.is_empty(), "{name}");
        assert!(!work.exists(), "{name}: nothing written");
    }
    // A download that differs from GitHub's digest is never attached.
    let mut release = dmg_release(&image);
    release.sha256 = hex(b"other bytes");
    let (result, opened, calls, work) = stage("network-mac-digest", updater::RS_BUNDLE_ID, release);
    assert_eq!(result, Err("내려받은 파일이 깃헙에 올라간 파일과 다릅니다.".to_string()));
    assert_eq!(opened.len(), 1);
    assert!(calls.is_empty(), "not attached");
    assert_eq!(std::fs::read_dir(work.join("download")).unwrap().count(), 0, "nothing unverified is left");
    // A disk image whose bundle carries the Python Grainy's identifier: attached, refused, detached.
    let (result, _, calls, work) = stage("network-mac-identifier", "io.github.pastorius9.grainy", dmg_release(&image));
    assert_eq!(result, Err(bad.to_string()));
    assert_eq!(calls, ["attach GrainyRS-0.2.0-macos-universal.dmg", "detach"]);
    assert!(!work.join("new").exists());
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_finish_step_reads_its_command_line() {
    let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(network::finish_arguments(&args(&["--finish-update", "generated", "--wrong", "42"])).is_none());
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
