//! Behaviour tests mirroring the oracle's own tests (test_safe_start, test_updater, test_feedback,
//! test_whats_new) in temporary folders, with real child processes where the oracle uses them.

mod common;

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use common::*;
use grainy_platform::feedback::{self, Outbox};
use grainy_platform::json::Value;
use grainy_platform::updater::{self, UpdateError};
use grainy_platform::{safe_start, wait_for_exit, Preferences};

fn spawn_waiting() -> Child {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("ping");
        c.args(["-n", "30", "127.0.0.1"]);
        c
    } else {
        let mut c = Command::new("sleep");
        c.arg("30");
        c
    };
    command.stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
}

fn spawn_gone() -> u32 {
    let mut command = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "exit"]);
        c
    } else {
        Command::new("true")
    };
    let mut child = command.stdout(Stdio::null()).spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

fn marker(folder: &Path, text: &str) {
    std::fs::write(folder.join(safe_start::NAME), text).unwrap();
}

#[test]
fn the_marker_tells_whether_the_run_before_closed() {
    let temp = Temp::new();
    let folder = &temp.0;
    assert!(safe_start::begin(folder, "0.6.8").is_none()); // first start
    let written = Value::parse(&std::fs::read_to_string(folder.join(safe_start::NAME)).unwrap()).unwrap();
    assert_eq!(written.as_object().unwrap().get("pid"), Some(&Value::Int(i128::from(std::process::id()))));
    safe_start::end(folder);
    assert!(!folder.join(safe_start::NAME).exists());
    assert!(safe_start::begin(folder, "0.6.8").is_none()); // closed normally: nothing left, nothing found
    let gone = spawn_gone();
    marker(folder, &format!("{{\"pid\": {gone}, \"version\": \"0.6.6\"}}"));
    let previous = safe_start::begin(folder, "0.6.8").expect("a crash is found");
    assert_eq!(previous.get("version"), Some(&Value::from("0.6.6"))); // its process is gone without closing
    let mut running = spawn_waiting();
    marker(folder, &format!("{{\"pid\": {}}}", running.id()));
    assert!(safe_start::begin(folder, "0.6.8").is_none(), "another Grainy still running is not a crash");
    running.kill().unwrap();
    running.wait().unwrap();
    marker(folder, "{\"pid\": -5}");
    safe_start::end(folder); // not this run's marker: left alone
    assert!(folder.join(safe_start::NAME).exists());
    marker(folder, "not json");
    assert!(safe_start::begin(folder, "0.6.8").is_none());
}

#[test]
fn end_removes_only_this_runs_marker() {
    let temp = Temp::new();
    marker(&temp.0, "{\"pid\": 4242}");
    safe_start::end_as(&temp.0, 4243);
    assert!(temp.0.join(safe_start::NAME).exists());
    safe_start::end_as(&temp.0, 4242);
    assert!(!temp.0.join(safe_start::NAME).exists());
    marker(&temp.0, "[1]");
    safe_start::end_as(&temp.0, 1);
    assert!(temp.0.join(safe_start::NAME).exists(), "a damaged marker is left alone");
}

#[test]
fn waiting_for_a_process_to_exit() {
    let mut child = spawn_waiting();
    assert!(!wait_for_exit(i64::from(child.id()), Duration::from_millis(50)), "still running");
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(wait_for_exit(i64::from(child.id()), Duration::from_secs(5)), "gone after it was reaped");
    assert!(wait_for_exit(0, Duration::ZERO));
    assert!(wait_for_exit(-3, Duration::ZERO));
}

#[test]
fn a_release_feed_is_read_from_a_file_or_through_the_opener_and_nothing_else() {
    let temp = Temp::new();
    let layout = windows_layout(&temp.0);
    let payload = serde_json::json!({
        "draft": false, "prerelease": false, "body": "notes",
        "assets": [{"name": "Grainy-9.9.9-windows-x64.zip", "size": 10,
                    "digest": format!("sha256:{}", "ab".repeat(32)),
                    "browser_download_url": "https://example.com/x.zip"}],
    });
    let feed = temp.0.join("feed.json");
    std::fs::write(&feed, payload.to_string()).unwrap();
    let never = |_: &str| -> std::io::Result<Box<dyn std::io::Read>> { panic!("no network in tests") };
    // a local file replaces the feed and then any download address is accepted
    let release = updater::latest(&never, Some(feed.to_str().unwrap()), &layout).unwrap().unwrap();
    assert_eq!(release.version, "9.9.9");
    // the real feed address only trusts downloads of this project: the opener is asked for the feed, nothing else
    let seen = std::cell::RefCell::new(Vec::new());
    let opener = |url: &str| -> std::io::Result<Box<dyn std::io::Read>> {
        seen.borrow_mut().push(url.to_string());
        Ok(Box::new(std::io::Cursor::new(payload.to_string().into_bytes())))
    };
    assert_eq!(updater::latest(&opener, None, &layout).unwrap(), None, "a foreign host is not trusted");
    assert_eq!(*seen.borrow(), [updater::ORACLE_FEED.to_string()]);
    let broken = |_: &str| -> std::io::Result<Box<dyn std::io::Read>> {
        Ok(Box::new(std::io::Cursor::new(b"not json".to_vec())))
    };
    assert!(matches!(updater::latest(&broken, None, &layout), Err(UpdateError::Value(_))));
    let offline = |_: &str| -> std::io::Result<Box<dyn std::io::Read>> { Err(std::io::Error::other("offline")) };
    assert!(matches!(updater::latest(&offline, None, &layout), Err(UpdateError::Io(_))));
}

#[test]
fn a_download_can_be_cancelled_and_leaves_nothing() {
    let temp = Temp::new();
    let source = temp.0.join("s.zip");
    std::fs::write(&source, vec![1u8; 3 << 20]).unwrap();
    let release = updater::Release {
        version: "1".into(),
        name: "Grainy-1.0-windows-x64.zip".into(),
        url: source.display().to_string(),
        size: 3 << 20,
        sha256: "0".repeat(64),
        notes: String::new(),
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut blocks = 0;
    let mut progress = |_: u64, _: u64| {
        blocks += 1;
        cancel.store(true, std::sync::atomic::Ordering::SeqCst); // cancelled after the first block
    };
    let never = |_: &str| -> std::io::Result<Box<dyn std::io::Read>> { panic!("no network") };
    let got = updater::download(&release, &temp.0.join("d"), Some(&mut progress), Some(&cancel), &never).unwrap();
    assert!(got.is_none());
    assert_eq!(blocks, 1);
    assert!(listing(&temp.0.join("d"), None).is_empty(), "no .part file left");
}

#[test]
fn finish_logged_swaps_and_writes_the_log() {
    let temp = Temp::new();
    let layout = windows_layout(&temp.0);
    let app = temp.0.join("app");
    put(
        &app,
        &[
            ("Grainy.exe".into(), "exe 1.0".into()),
            ("_internal/lib.dll".into(), "lib 1.0".into()),
            ("my-notes.txt".into(), "mine".into()),
        ],
    );
    zip_file(
        &temp.0.join("n.zip"),
        &[("Grainy/Grainy.exe".into(), "exe 2".into()), ("Grainy/_internal/a.dll".into(), "a".into())],
    );
    let staged = updater::stage(&temp.0.join("n.zip"), &app.join(".update"), &layout).unwrap();
    assert_eq!(updater::finish_logged(&app, &staged, 0, &layout), "");
    assert_eq!(std::fs::read_to_string(app.join("Grainy.exe")).unwrap(), "exe 2");
    assert_eq!(std::fs::read_to_string(app.join("my-notes.txt")).unwrap(), "mine");
    let log = std::fs::read_to_string(app.join(".update").join("update.log")).unwrap();
    assert!(log.contains("new version copied"));
}

#[test]
fn finish_does_not_swap_while_the_old_app_still_runs() {
    let temp = Temp::new();
    let layout = windows_layout(&temp.0);
    let app = temp.0.join("app");
    put(&app, &[("Grainy.exe".into(), "exe 1.0".into())]);
    put(&app.join(".update").join("new"), &[("Grainy.exe".into(), "exe 2".into())]);
    let mut child = spawn_waiting();
    let hooks = updater::Hooks { exit_wait: Duration::from_millis(150), ..updater::Hooks::default() };
    let result =
        updater::finish_with(&app, &app.join(".update").join("new"), i64::from(child.id()), None, &layout, &hooks);
    child.kill().unwrap();
    child.wait().unwrap();
    assert!(!result.is_empty());
    assert_eq!(std::fs::read_to_string(app.join("Grainy.exe")).unwrap(), "exe 1.0");
    assert!(
        !app.join(".update").exists()
            || !listing(&app.join(".update"), None).keys().any(|k| k.starts_with("previous-"))
    );
}

#[test]
fn the_recycle_bin_takes_a_folder() {
    let temp = Temp::new();
    let victim = temp.0.join("victim-folder");
    std::fs::create_dir_all(&victim).unwrap();
    std::fs::write(victim.join("a.txt"), "x").unwrap();
    if updater::recycle(&victim) {
        assert!(!victim.exists());
    } else {
        println!("no recycle bin on this machine: skipped");
    }
}

#[test]
fn the_free_disk_space_can_be_read() {
    let temp = Temp::new();
    assert!(updater::free_space(&temp.0).unwrap() > 0);
}

#[test]
fn preferences_reject_unknown_languages_and_report_defaults() {
    let temp = Temp::new();
    let mut prefs = Preferences::new(temp.0.join("s.json"));
    assert!(prefs.language().is_none() && prefs.use_gpu());
    assert!(prefs.save_language("fr").is_err());
    assert!(!temp.0.join("s.json").exists());
    prefs.save_language("en").unwrap();
    assert_eq!(Preferences::new(temp.0.join("s.json")).language().map(|l| l.code()), Some("en"));
}

#[test]
fn another_writer_between_two_saves_is_merged_not_overwritten() {
    let temp = Temp::new();
    let path = temp.0.join("s.json");
    let mut mine = Preferences::new(&path);
    mine.save("a", Value::Int(1)).unwrap();
    let mut other = Preferences::new(&path); // the feedback dialog's own object
    other.save("feedback_sent", Value::List(vec![])).unwrap();
    mine.save("b", Value::Int(2)).unwrap();
    let again = Preferences::new(&path);
    let keys: Vec<&str> = again.values.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(keys, ["a", "feedback_sent", "b"]);
}

#[test]
fn the_settings_default_location_follows_the_environment() {
    let env = grainy_platform::Environment {
        system: grainy_platform::System::Windows,
        local_app_data: Some("X:/Local".into()),
        home: "H:/home".into(),
    };
    let prefs = Preferences::at_default(&env);
    assert_eq!(prefs.path.to_string_lossy().replace('\\', "/"), "X:/Local/Grainy/settings.json");
}

#[test]
fn references_come_from_the_alphabet_and_a_failed_send_keeps_its_reference() {
    for _ in 0..200 {
        let reference = feedback::reference();
        assert_eq!(reference.chars().count(), 5);
        assert!(reference.chars().all(|c| feedback::ALPHABET.contains(c)));
    }
    let mut counter = 0;
    let fixed = feedback::reference_with(&mut |n| {
        counter += 1;
        counter % n
    });
    assert_eq!(fixed, "BCDEF");
    let mut outbox = Outbox::default();
    assert!(outbox.prepare("   ", "sys").is_none());
    let (text, tagged, first) = outbox.prepare("  hello ", "sys").unwrap();
    assert_eq!(text, "hello");
    assert_eq!(tagged, format!("sys · #{first}"));
    let (_, _, again) = outbox.prepare("hello", "sys").unwrap();
    assert_eq!(again, first, "the same text sent again keeps its reference");
    let (_, _, other) = outbox.prepare("different", "sys").unwrap();
    assert_ne!(other, first, "a new text gets a new reference");
    assert!(outbox.sent().is_some());
    let (_, _, fresh) = outbox.prepare("different", "sys").unwrap();
    assert_ne!(fresh, other, "after a successful send the next message starts over");
}
