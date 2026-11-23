//! Window, settings and the small dialogs (work package 26).
//!
//! Models against goldens made with the oracle's own code (`tools/golden/gen_app_settings.py`, `golden/app/settings*`):
//! the 설정 menu's items, `LanguageDialog`, `DesktopSession.change_language`, `whats_new.show`, `show_about`,
//! `show_help` and the learn link, `UpdateController`, `ScreenModes`, the preference writes and `drt_advanced`.
//! Window state (the Rust app's own file) and the screen with no photo are tested without a window; the real
//! window (egui_kittest on wgpu) draws the dialogs, the menu and lights out in Korean and English.

mod common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::settings::{self, UpdateState};
use grainy_app::dialogs::about;
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::views::compare::Compare;
use grainy_app::window_state::{self, WindowState};
use grainy_app::{AppContext, GrainyApp, Mode};
use grainy_i18n::Language;
use grainy_platform::preferences::Preferences;
use grainy_platform::{updater, whats_new, APP_VERSION, RS_VERSION};
use grainy_testkit::Case;
use serde_json::{json, Value};

fn turn() -> std::sync::MutexGuard<'static, ()> {
    common::language_lock()
}

fn language_of(code: &str) -> Language {
    Language::parse(code).unwrap()
}

fn context() -> AppContext {
    let mut session = Session::default();
    session.add([PathBuf::from("a.jpg"), PathBuf::from("b.jpg")]);
    AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(session))
}

fn with_data(name: &str) -> (AppContext, PathBuf) {
    let folder = common::temp_dir(name);
    let mut cx = context();
    cx.set_data_folder(Some(folder.clone()));
    (cx, folder)
}

fn stored(folder: &std::path::Path) -> Value {
    let text = std::fs::read_to_string(folder.join("settings.json")).unwrap_or_else(|_| "{}".into());
    serde_json::from_str(&text).unwrap()
}

// ---- the 설정 menu ---------------------------------------------------------------------------------------------------

/// The oracle's settings menu (the real window with `DesktopSession.open_window`'s additions): the items this
/// package owns exist as commands with the oracle's text, in the oracle's order, the update item checkable.
#[test]
fn the_settings_menu_has_the_oracles_items_in_its_order() {
    let g = Case::open("app", "settings_menu").json().clone();
    let rows = g["rows"].as_array().unwrap();
    let text = |row: &Value| row["text"].as_str().unwrap_or("").to_owned();
    let ours = [
        ("언어 / Language…", "settings.language"),
        ("단축키 목록…", "view.shortcuts"),
        ("Grainy 배우기 (웹 튜토리얼)…", "settings.learn"),
        ("새 버전 자동 확인", "settings.update_auto"),
        ("업데이트 확인…", "settings.update_check"),
        ("이번 버전의 달라진 점…", "settings.whats_new"),
        ("Grainy 정보…", "settings.about"),
    ];
    for (label, id) in ours {
        let row = rows.iter().find(|r| r["text"] == label).unwrap_or_else(|| panic!("the oracle has {label}"));
        let command = grainy_app::commands::find(id).unwrap_or_else(|| panic!("command {id}"));
        assert_eq!(command.label, label, "{id}");
        assert_eq!(row["checkable"], json!(id == "settings.update_auto"), "{label} checkable");
    }
    // The order of the oracle's rows and the order of the items in the program's menu.
    let source = include_str!("../src/menus.rs");
    let body = &source[source.find("fn settings_menu").unwrap()..];
    let mut last = 0;
    for row in rows {
        let name = text(row);
        if let Some((_, _)) = ours.iter().find(|(l, _)| *l == name) {
            let at = body.find(&format!("\"{name}\"")).unwrap_or_else(|| panic!("{name} is in the menu"));
            assert!(at > last, "{name} comes after the item before it");
            last = at;
        }
    }
    // The GPU item sits between the language and the folder item, as in the oracle.
    let names: Vec<String> = rows.iter().map(text).collect();
    let at = |label: &str| names.iter().position(|n| n == label).unwrap();
    assert!(at("언어 / Language…") < at("GPU 가속 사용") && at("GPU 가속 사용") < at("단축키 목록…"));
    // The separator before 이번 버전의 달라진 점…, after the update items.
    let separator = rows.iter().position(|r| r.get("separator").is_some()).unwrap();
    assert_eq!(separator + 1, at("이번 버전의 달라진 점…"));
    assert!(at("업데이트 확인…") < separator);
    let text_before = &body[..body.find("\"이번 버전의 달라진 점…\"").unwrap()];
    assert!(text_before.rfind("ui.separator()").unwrap() > text_before.find("\"업데이트 확인…\"").unwrap());
    // The shortcut list shows the key of the table.
    assert_eq!(rows[at("단축키 목록…")]["shortcut"], "? / Ctrl+/");
    assert_eq!(grainy_app::commands::keys("view.shortcuts").first().copied(), Some("Shift+/"));
    assert_eq!(g["learn_url"], settings::LEARN_URL);
}

// ---- the language box ------------------------------------------------------------------------------------------------

#[test]
fn the_language_dialog_says_what_the_oracles_says() {
    use grainy_app::dialogs::language::{LanguageDialog, LANGUAGES};
    let g = Case::open("app", "settings_language_dialog").json().clone();
    let cases = g["dialogs"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let (first_run, current) =
            (case["first_run"].as_bool().unwrap(), language_of(case["current"].as_str().unwrap()));
        let dialog = LanguageDialog::new(Some(current), first_run, false);
        assert_eq!(LanguageDialog::title(), case["title"]);
        assert_eq!(case["labels"], json!([dialog.heading(), dialog.note()]), "first_run {first_run}");
        assert_eq!(dialog.ok_text(), case["ok"]);
        assert_eq!(LanguageDialog::cancel_text(), case["cancel"]);
        let items: Vec<Value> = LANGUAGES.iter().map(|(t, l)| json!([t, l.code()])).collect();
        assert_eq!(Value::Array(items), case["items"]);
        assert_eq!(case["items"][case["index"].as_u64().unwrap() as usize][1], dialog.selected().code());
        assert_eq!(dialog.selected().code(), case["selected"]);
        assert_eq!(dialog.first_run(), first_run);
    }
    // Without a language in use the system's decides: Korean for a Korean system, English otherwise.
    assert_eq!(LanguageDialog::new(None, true, true).selected(), Language::Korean);
    assert_eq!(LanguageDialog::new(None, true, false).selected(), Language::English);
    for korean in ["ko", "ko-KR", "ko_KR.UTF-8", "Korean_Korea.949", " KO "] {
        assert!(settings::is_korean_locale(korean), "{korean}");
    }
    for other in ["en-US", "ja_JP", "", "C", "kok-IN"] {
        assert!(!settings::is_korean_locale(other), "{other}");
    }
}

/// `DesktopSession.change_language`: the same language does nothing, a running job refuses and says so, a change
/// stores the preference and keeps the photo, the screen and the panels.
#[test]
fn changing_the_language_follows_the_oracle() {
    let _turn = turn();
    let g = Case::open("app", "settings_language_change").json().clone();
    let steps = g["steps"].as_array().unwrap();
    let step = |name: &str| steps.iter().find(|s| s["name"] == name).unwrap();
    let (mut cx, folder) = with_data("settings-language-change");
    grainy_i18n::set_language(Language::Korean);
    cx.select_photo(1);
    cx.set_mode(Mode::Library);
    cx.set_expanded_panels(&["basic", "color"]);
    let (photo, panels) = (cx.doc.current_id(), cx.expanded_panels().to_vec());

    // Same language.
    assert_eq!(settings::change_language(&mut cx, language_of("ko")), step("same")["result"]);
    assert_eq!(stored(&folder).get("language").cloned(), step("same")["stored_language"].as_str().map(Value::from));
    // A job runs: the window refuses, nothing is stored.
    let release = Arc::new(AtomicBool::new(false));
    let waiting = release.clone();
    cx.jobs.spawn(
        "export",
        "내보내는 중…",
        move |_| {
            while !waiting.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(())
        },
        |_: &mut AppContext, _| {},
    );
    assert_eq!(settings::change_language(&mut cx, language_of("en")), step("busy export")["result"]);
    assert_eq!(cx.status(), grainy_i18n::translate("작업이 끝난 뒤 언어를 변경해 주세요.", Language::Korean));
    assert_eq!(grainy_i18n::language(), Language::Korean);
    assert!(stored(&folder).get("language").is_none(), "{}", step("busy export")["stored_language"]);
    release.store(true, Ordering::Relaxed);
    while !cx.jobs.jobs_idle() {
        settings::pump(&mut cx);
        std::thread::sleep(Duration::from_millis(5));
    }
    // A change.
    assert_eq!(settings::change_language(&mut cx, language_of("en")), step("change")["result"]);
    assert_eq!(grainy_i18n::language().code(), step("change")["language"]);
    assert_eq!(stored(&folder)["language"], step("change")["stored_language"]);
    assert_eq!(cx.doc.current_id(), photo, "the photo stays");
    assert_eq!(cx.mode(), Mode::Library, "the screen stays");
    assert_eq!(cx.expanded_panels(), panels.as_slice(), "the panels stay");
    assert_eq!(
        cx.status(),
        grainy_i18n::translate("준비됨  ·  사진을 가져오면 시작할 수 있습니다.", Language::English)
    );
    // The oracle hands the saved geometry, photo and screen to the new window.
    assert_eq!(step("change")["calls"][0]["geometry"], true);
    assert!(Preferences::new(folder.join("settings.json")).language() == Some(Language::English));
    grainy_i18n::set_language(Language::Korean);
}

/// The first-run question stores the language it was answered with and shows what's new only for a shipped
/// program that was updated; without a data folder nothing is stored.
#[test]
fn the_first_run_question_stores_the_language() {
    let _turn = turn();
    let (mut cx, folder) = with_data("settings-first-run");
    grainy_i18n::set_language(Language::Korean);
    settings::startup(&mut cx);
    assert!(cx.dialog_open("language"), "no language is stored yet: the question opens");
    settings::language_chosen(&mut cx, true, Language::English);
    assert_eq!(grainy_i18n::language(), Language::English);
    assert_eq!(stored(&folder)["language"], "en");
    // A second start knows the language and does not ask.
    grainy_i18n::set_language(Language::Korean);
    settings::apply_stored_language(&folder);
    assert_eq!(grainy_i18n::language(), Language::English);
    let (mut again, _) = (context(), ());
    again.set_data_folder(Some(folder.clone()));
    settings::startup(&mut again);
    assert!(!again.dialog_open("language"));
    // A language that is not stored leaves the default.
    grainy_i18n::set_language(Language::Korean);
    let other = common::temp_dir("settings-no-language");
    settings::apply_stored_language(&other);
    assert_eq!(grainy_i18n::language(), Language::Korean);
    // No data folder: nothing to read or write, nothing asked.
    let mut memory = context();
    settings::startup(&mut memory);
    assert!(!memory.dialog_open("language"));
    settings::language_chosen(&mut memory, true, Language::English);
    assert_eq!(grainy_i18n::language(), Language::English);
    grainy_i18n::set_language(Language::Korean);
}

// ---- what's new, about, help ------------------------------------------------------------------------------------------

/// The oracle's change log: the Python Grainy's own, which the goldens were made from. Grainy RS never shows it.
fn oracle_notes() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../grainy-platform/assets/root")
}

/// The box lays out what the oracle's box does; its title and heading name Grainy RS and its own version where the
/// oracle names itself and 0.6.8.
#[test]
fn whats_new_says_what_the_oracles_says() {
    use grainy_app::dialogs::whats_new::WhatsNew;
    let _turn = turn();
    let g = Case::open("app", "settings_whats_new").json().clone();
    assert_eq!(g["version"], APP_VERSION);
    let as_oracle = |text: &str| text.replace("Grainy RS", "Grainy").replace(RS_VERSION, APP_VERSION);
    for case in g["cases"].as_array().unwrap() {
        grainy_i18n::set_language(language_of(case["language"].as_str().unwrap()));
        let dialog = match case["name"].as_str().unwrap() {
            "this version" => WhatsNew::new(whats_new::notes(None, APP_VERSION, None, &oracle_notes()), Vec::new()),
            "none" => WhatsNew::new(Vec::new(), Vec::new()),
            _ => WhatsNew::new(Vec::new(), vec!["AB12".into(), "CD34".into()]),
        };
        let texts = dialog.texts();
        let name = format!("{} {}", case["language"], case["name"]);
        assert_eq!(as_oracle(&texts.title), case["title"], "{name}");
        assert!(texts.title.contains(&format!("Grainy RS {RS_VERSION}")), "{name}: {}", texts.title);
        let labels = case["labels"].as_array().unwrap();
        let label = |object: &str| labels.iter().find(|l| l[0] == object).map(|l| l[1].as_str().unwrap().to_owned());
        assert_eq!(Some(as_oracle(&texts.heading)), label(""), "{name}");
        assert!(texts.heading.contains("Grainy RS") && texts.heading.contains(RS_VERSION), "{name}: {}", texts.heading);
        assert_eq!(texts.thanks, label("shipped"), "{name}");
        assert_eq!(texts.thanks.is_some(), case["thanks"].is_string(), "{name}");
        assert_eq!(texts.markdown, case["markdown"].as_str().unwrap(), "{name}");
    }
    grainy_i18n::set_language(Language::Korean);
}

/// 이번 버전의 달라진 점… shows Grainy RS's own notes of its own version, in Korean and in English; the oracle's
/// change log (the Python Grainy's 0.6.8) is read only the oracle's way, for the goldens.
#[test]
fn the_notes_of_this_version_are_grainy_rs_own() {
    use grainy_app::dialogs::whats_new::{self as whats_new_box, WhatsNew};
    let _turn = turn();
    let oracle_ko = whats_new::notes(None, APP_VERSION, Some(Language::Korean), &oracle_notes());
    let oracle_en = whats_new::notes(None, APP_VERSION, Some(Language::English), &oracle_notes());
    assert_eq!(oracle_ko.len(), 1);
    assert_eq!(oracle_ko[0].version, APP_VERSION);
    assert!(oracle_ko[0].title.contains("안전 시작") && oracle_en[0].title.contains("Safe start"));

    let korean = whats_new_box::notes(None, RS_VERSION, Some(Language::Korean));
    let english = whats_new_box::notes(None, RS_VERSION, Some(Language::English));
    assert_eq!(korean.len(), 1, "assets/whats-new/ko.md has a section for {RS_VERSION}");
    assert_eq!(english.len(), 1);
    assert_eq!((korean[0].version.as_str(), english[0].version.as_str()), (RS_VERSION, RS_VERSION));
    assert_ne!(korean[0].body, english[0].body, "assets/whats-new/en.md has an English section for {RS_VERSION}");
    for (language, entry, title) in [
        (Language::Korean, &korean[0], format!("Grainy RS {RS_VERSION}의 달라진 점")),
        (Language::English, &english[0], format!("What's new in Grainy RS {RS_VERSION}")),
    ] {
        grainy_i18n::set_language(language);
        let texts = WhatsNew::this_version().texts();
        assert_eq!(texts.title, title);
        assert_eq!(texts.markdown, whats_new::markdown(std::slice::from_ref(entry)));
        assert!(texts.markdown.starts_with(&format!("## {RS_VERSION} \u{2014} ")), "{}", texts.markdown);
        for oracle in [&oracle_ko[0], &oracle_en[0]] {
            assert!(!texts.markdown.contains(&oracle.title), "the Python Grainy's notes: {}", oracle.title);
        }
    }
    grainy_i18n::set_language(Language::Korean);
}

/// Every Grainy RS release (its GitHub notes in `docs/releases`) has its section in both in-app change logs.
#[test]
fn every_release_has_in_app_notes() {
    use grainy_app::dialogs::whats_new::{NOTES_EN, NOTES_KO};
    let versions = |text: &str| -> Vec<String> { whats_new::sections(text).into_iter().map(|e| e.version).collect() };
    let (korean, english) = (versions(NOTES_KO), versions(NOTES_EN));
    assert_eq!(korean, english, "ko.md and en.md list the same versions in the same order");
    let releases = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/releases");
    let mut released: Vec<String> = std::fs::read_dir(&releases)
        .unwrap()
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "md").then(|| path.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect();
    released.sort();
    assert!(released.len() >= 5, "{released:?}");
    for version in &released {
        assert!(korean.contains(version), "docs/releases/{version}.md has no section in assets/whats-new/ko.md");
    }
}

#[test]
fn the_about_box_says_what_the_oracles_says() {
    let _turn = turn();
    let g = Case::open("app", "settings_about").json().clone();
    for case in g["about"].as_array().unwrap() {
        grainy_i18n::set_language(language_of(case["language"].as_str().unwrap()));
        let message = grainy_app::dialogs::about::about_box();
        assert_eq!(message.title, case["box"]["title"]);
        // The version line names the Rust build and its own version.
        assert_eq!(case["box"]["text"], format!("Grainy {APP_VERSION}"));
        assert_eq!(message.text, format!("Grainy RS {RS_VERSION}"));
        // The oracle's notice, then the Lensfun attribution the Rust app adds for its built-in lens database.
        let oracle = case["box"]["informative"].as_str().unwrap();
        assert_eq!(message.informative, format!("{oracle}\n\n{}", grainy_app::text::tr(about::LENSFUN_NOTICE)));
        assert!(message.informative.contains("CC BY-SA 3.0") && message.informative.contains("Lensfun"));
        assert!(message.detail.is_empty());
    }
    grainy_i18n::set_language(Language::Korean);
    assert_eq!(g["version"], APP_VERSION);
}

#[test]
fn help_and_the_learn_link_open_what_the_oracle_opens() {
    let _turn = turn();
    let g = Case::open("app", "settings_about").json().clone();
    let recorded = || std::mem::take(&mut *settings::LAUNCHED.lock().unwrap());
    grainy_app::commands::library_column::LAUNCH_DISABLED.store(true, Ordering::Relaxed);
    recorded();
    let mut cx = context();
    assert!(cx.run("settings.learn"));
    let g2 = Case::open("app", "settings_menu").json().clone();
    assert_eq!(recorded(), [g2["learn_url"].as_str().unwrap().to_owned()]);
    // README.html next to the program (a folder above it): a file, as the oracle opens a local file.
    let folder = common::temp_dir("settings-help");
    std::fs::write(folder.join("README.html"), "<html></html>").unwrap();
    let target = settings::help_target(&[folder.join("missing"), folder.clone()]);
    assert!(target.ends_with(g["help_file"].as_str().unwrap()), "{target}");
    assert!(std::path::Path::new(&target).is_absolute() && g["help_scheme"] == "file");
    // Without the file the tutorial page opens.
    assert_eq!(settings::help_target(&[folder.join("missing")]), settings::LEARN_URL);
    assert!(cx.run("settings.help"));
    assert_eq!(recorded().len(), 1);
    grainy_app::commands::library_column::LAUNCH_DISABLED.store(false, Ordering::Relaxed);
}

// ---- updates --------------------------------------------------------------------------------------------------------

fn release_of(value: &Value) -> updater::Release {
    updater::Release {
        version: value["version"].as_str().unwrap().into(),
        name: value["name"].as_str().unwrap().into(),
        url: value["url"].as_str().unwrap().into(),
        size: value["size"].as_u64().unwrap(),
        sha256: value["sha256"].as_str().unwrap().into(),
        notes: value["notes"].as_str().unwrap().into(),
    }
}

/// `UpdateController.checked` after the results of the oracle's scenarios: the boxes (title, text, buttons), the
/// status line, the stored time of the check and the release kept for the header's button.
#[test]
fn update_results_say_what_the_oracles_say() {
    let _turn = turn();
    let g = Case::open("app", "settings_updates").json().clone();
    assert_eq!(g["version"], APP_VERSION);
    let newer = release_of(&g["release"]);
    // The oracle's texts name its version; the Rust build names its own (`RS_VERSION`).
    assert!(!newer.version.contains(APP_VERSION));
    let rs = |value: &Value| Value::from(value.as_str().unwrap().replace(APP_VERSION, RS_VERSION));
    for case in g["results"].as_array().unwrap() {
        let language = language_of(case["language"].as_str().unwrap());
        grainy_i18n::set_language(language);
        let name = format!("{} {}", case["language"], case["name"]);
        let manual = case["manual"].as_bool().unwrap();
        let (mut cx, folder) = with_data("settings-update");
        cx.state::<UpdateState>().packaged = Some(false);
        let result: Result<Option<updater::Release>, String> =
            match case["name"].as_str().unwrap().split(' ').next().unwrap() {
                "error" => Err("boom".into()),
                "same" => Ok(Some(updater::Release { version: RS_VERSION.into(), ..newer.clone() })),
                "none" => Ok(None),
                _ => Ok(Some(newer.clone())),
            };
        settings::checked(&mut cx, manual, result.clone());
        let shown = case["shown"].as_array().unwrap();
        // A box opens exactly when the oracle shows one.
        assert_eq!(cx.dialogs.len(), shown.len(), "{name}");
        if let Some(first) = shown.first() {
            let (id, message) = match first["kind"].as_str().unwrap() {
                "offer" => ("update_offer", settings::offer_box(&newer, false, None)),
                _ if result.is_err() => ("update", settings::error_box("boom")),
                _ => ("update", settings::latest_box()),
            };
            assert!(cx.dialog_open(id), "{name}");
            assert_eq!(message.title, first["title"], "{name}");
            assert_eq!(message.text, rs(&first["text"]), "{name}");
            if first["kind"] == "offer" {
                assert_eq!(message.informative, first["informative"], "{name}");
                assert_eq!(message.detail, first["detailed"], "{name}");
                let mut ours: Vec<&str> = message.buttons.iter().map(|(n, _)| n.as_str()).collect();
                let mut theirs: Vec<&str> = first["buttons"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|b| b.as_str().unwrap())
                    .filter(|b| !b.starts_with("Show Details"))
                    .collect();
                ours.sort_unstable();
                theirs.sort_unstable();
                assert_eq!(ours, theirs, "{name}: the buttons (no 지금 업데이트 for a program run from source)");
            }
        }
        let status = case["status"].as_str().unwrap();
        if manual
            && !status.is_empty()
            && case["name"] != "newer manual"
            && case["name"] != "error manual"
            && !status.contains("확인하는 중")
            && !status.contains("Checking")
        {
            assert_eq!(cx.status(), status, "{name}");
        }
        if case["name"] == "newer auto" {
            assert_eq!(cx.status(), status, "{name}");
        }
        let kept = cx.state::<UpdateState>().release.clone();
        assert_eq!(
            kept.is_some(),
            case["button_text"].as_str().is_some_and(|t| !t.is_empty()) && !case["button_hidden"].as_bool().unwrap(),
            "{name}"
        );
        if let Some(kept) = kept {
            assert_eq!(case["button_text"], grainy_i18n::tr_with("새 버전 {0}", &[&kept.version]), "{name}");
        }
        assert_eq!(stored(&folder).get("update_checked").is_some(), case["update_checked_saved"], "{name}");
    }
    grainy_i18n::set_language(Language::Korean);
}

/// The menu's checkable item stores `update_check` as the oracle's action does.
#[test]
fn the_automatic_check_item_stores_its_preference() {
    let g = Case::open("app", "settings_updates").json().clone();
    let (mut cx, folder) = with_data("settings-update-auto");
    assert!(!settings::update_check_enabled(&cx));
    for want in g["toggles"].as_array().unwrap() {
        assert!(cx.run("settings.update_auto"));
        assert_eq!(&stored(&folder)["update_check"], want);
        assert_eq!(settings::update_check_enabled(&cx), want.as_bool().unwrap());
    }
}

/// A manual check reads the feed on a job: a local feed file (the oracle's `GRAINY_UPDATE_FEED`) with a newer
/// release offers it, with the same version says so, without the network the failure box opens.
#[test]
fn a_check_reads_the_feed_on_a_job() {
    let _turn = turn();
    grainy_i18n::set_language(Language::Korean);
    let layout = updater::Layout::current();
    // This system's file of the Rust build: the Windows zip, or on macOS the universal disk image.
    let tail = updater::rust_asset_pattern(&layout).tail().to_owned();
    let feed = |version: &str| {
        json!({"draft": false, "prerelease": false, "body": "새 기능", "assets": [{
            // The Rust build's own release file (`updater::RUST_MARKER`; a Python `Grainy-` file never qualifies).
            "name": format!("{}{version}{tail}", updater::RUST_MARKER), "size": 1234, "digest": format!("sha256:{}", "ab".repeat(32)),
            "browser_download_url": format!("{}v{version}/{}{version}{tail}", updater::RS_DOWNLOADS, updater::RUST_MARKER)}]})
    };
    let folder = common::temp_dir("settings-feed");
    let run = |cx: &mut AppContext| {
        settings::check_for_update(cx, true);
        assert!(cx.status().contains("확인하는 중"), "the status line says a check runs");
        let started = Instant::now();
        loop {
            settings::pump(cx);
            if cx.jobs.jobs_idle() {
                break;
            }
            assert!(started.elapsed() < Duration::from_secs(20));
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    for (version, want) in [("9.9.9", "update_offer"), (RS_VERSION, "update")] {
        let path = folder.join(format!("feed-{version}.json"));
        std::fs::write(&path, feed(version).to_string()).unwrap();
        let (mut cx, _) = with_data("settings-feed-cx");
        cx.state::<UpdateState>().feed = Some(path.to_string_lossy().into_owned());
        cx.state::<UpdateState>().packaged = Some(false);
        run(&mut cx);
        assert!(cx.dialog_open(want), "{version}");
        assert_eq!(cx.state::<UpdateState>().release.is_some(), version == "9.9.9");
    }
    // The network feed without a way to read it: the failure box, and a second check may start afterwards.
    let (mut cx, _) = with_data("settings-feed-none");
    let missing = folder.join("missing.json");
    cx.state::<UpdateState>().feed = Some(missing.to_string_lossy().into_owned());
    run(&mut cx);
    assert!(cx.dialog_open("update"));
    settings::check_for_update(&mut cx, false);
    assert!(!cx.jobs.jobs_idle(), "a new check starts when the last one ended");
    let started = Instant::now();
    while !cx.jobs.jobs_idle() || started.elapsed() < Duration::from_millis(50) {
        settings::pump(&mut cx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ---- lights out and the screen modes --------------------------------------------------------------------------------

/// `ScreenModes` on the oracle's stand-in window: after every step of each script, the visible parts, the flags
/// and what Esc did equal the oracle's.
#[test]
fn screen_modes_follow_the_oracle() {
    let g = Case::open("app", "settings_screen_modes").json().clone();
    for run in g["runs"].as_array().unwrap() {
        let name = run["name"].as_str().unwrap();
        let mut cx = context();
        cx.set_mode(if run["develop"] == true { Mode::Develop } else { Mode::Library });
        cx.state::<Compare>().on = run["compare"] == true;
        for (index, row) in run["rows"].as_array().unwrap().iter().enumerate() {
            let step = row["step"].as_str().unwrap();
            let mut result = None;
            match step {
                "start" => {}
                "sides" => assert!(cx.run("view.sides")),
                "bars" => assert!(cx.run("view.bars")),
                "lights" => assert!(cx.run("view.lights")),
                "full" => assert!(cx.run("view.full_screen")),
                "leave" => result = Some(settings::leave(&mut cx)),
                other => panic!("step {other}"),
            }
            let at = format!("{name} step {index} ({step})");
            assert_eq!(row["result"].as_bool(), result, "{at}: leave");
            // The oracle keeps the *hidden* flags.
            assert_eq!(row["sides"], !cx.sides_visible(), "{at}: sides");
            assert_eq!(row["bars"], !cx.bars_visible(), "{at}: bars");
            assert_eq!(row["lights"], settings::lights_on(&cx), "{at}: lights");
            assert_eq!(row["full"], cx.full_screen(), "{at}: full screen");
            let v = settings::visible_now(&cx);
            let want = &row["visible"];
            let have = json!({
                "library_column": v.library_column, "edit_column": v.edit_column, "header_bar": v.header_bar,
                "view_bar": v.view_bar, "filmstrip": v.filmstrip, "library_toolbar": v.library_toolbar,
                "compare_bar": v.compare_bar, "menu": v.menu, "status": v.status,
            });
            assert_eq!(&have, want, "{at}: visible parts");
        }
    }
}

// ---- the preferences ------------------------------------------------------------------------------------------------

/// The settings file after the preference writes of this package equals the file the oracle's `Preferences`
/// leaves (values and key order) after the same writes.
#[test]
fn the_settings_file_equals_the_oracles_after_the_same_writes() {
    let _turn = turn();
    grainy_i18n::set_language(Language::Korean);
    let g = Case::open("app", "settings_preferences").json().clone();
    let trace = g["trace"].as_array().unwrap();
    let (mut cx, folder) = with_data("settings-prefs");
    let order = |folder: &std::path::Path| -> Vec<String> {
        Preferences::new(folder.join("settings.json")).values.iter().map(|(k, _)| k.clone()).collect()
    };
    // The window's own key (`window`) and the Grainy RS version opened (`rs_version_seen`) are the Rust app's
    // additions; the oracle's keys are compared.
    let own = |key: &str| key == "window" || key == grainy_app::dialogs::whats_new::SEEN;
    let check = |cx: &AppContext, step: &str| {
        let folder = cx.data_folder().unwrap();
        let want = trace.iter().find(|t| t["step"] == step).unwrap();
        let mut file = stored(folder);
        file.as_object_mut().unwrap().retain(|key, _| !own(key));
        assert_eq!(file, want["file"], "{step}");
        let keys: Vec<String> = order(folder).into_iter().filter(|k| !own(k)).collect();
        assert_eq!(Value::from(keys), want["order"], "{step}: key order");
    };
    assert!(settings::change_language(&mut cx, Language::English));
    check(&cx, "language en");
    assert!(cx.run("settings.update_auto"));
    check(&cx, "update_check on");
    assert!(cx.run("settings.update_auto"));
    check(&cx, "update_check off");
    // The version just opened (a shipped program, nothing seen before): the notes open and the version is kept.
    assert!(settings::whats_new_on_start(&mut cx, false, true));
    assert!(cx.dialog_open("whats_new"));
    check(&cx, "version_seen");
    Preferences::new(folder.join("settings.json")).save_use_gpu(false).unwrap();
    check(&cx, "gpu off");
    assert!(settings::change_language(&mut cx, Language::Korean));
    check(&cx, "language ko");
    grainy_i18n::set_language(Language::Korean);
    assert_eq!(g["rejected"], true);
}

/// `whats_new.pending`: the oracle's choice over its own change log is the golden's; the start of the program makes
/// the same choice with Grainy RS's version and notes, remembered under its own key, and opens nothing for a first
/// run, a program run from source, the same version or a downgrade.
#[test]
fn whats_new_opens_for_the_versions_the_oracle_lists() {
    use grainy_app::dialogs::whats_new as whats_new_box;
    let _turn = turn();
    grainy_i18n::set_language(Language::Korean);
    let g = Case::open("app", "settings_preferences").json().clone();
    let pending = &g["pending"];
    let oracle = |seen: &str| -> Value {
        let file = common::temp_dir("settings-pending-oracle").join("settings.json");
        Preferences::new(file.clone()).save(whats_new::KEY, seen.into()).unwrap();
        let notes = whats_new::pending(&Preferences::new(file), false, APP_VERSION, None, &oracle_notes());
        Value::from(notes.iter().map(|e| e.version.clone()).collect::<Vec<_>>())
    };
    assert_eq!(oracle("0.6.5"), pending["from 0.6.5"]);
    assert_eq!(oracle("9.0.0"), pending["downgrade"]);
    assert_eq!(oracle(APP_VERSION), pending["same version"]);

    // A data folder holding only the language, and the given versions opened before.
    let start = |seen: &[(&str, &str)]| {
        let (cx, folder) = with_data("settings-pending");
        let mut prefs = Preferences::new(folder.join("settings.json"));
        prefs.save("language", "ko".into()).unwrap();
        for (key, version) in seen {
            prefs.save(key, (*version).into()).unwrap();
        }
        let notes = whats_new_box::pending(&Preferences::new(folder.join("settings.json")), false, RS_VERSION, None);
        (notes.into_iter().map(|e| e.version).collect::<Vec<_>>(), cx)
    };
    // Nothing remembered (a fresh start, or an older Grainy RS that kept only the oracle's 0.6.8): this version.
    for seen in [&[][..], &[("version_seen", APP_VERSION)][..]] {
        let (listed, mut cx) = start(seen);
        assert_eq!(listed, [RS_VERSION]);
        assert!(settings::whats_new_on_start(&mut cx, false, true));
        assert!(cx.dialog_open("whats_new"));
        let file = stored(cx.data_folder().unwrap());
        assert_eq!(file[whats_new_box::SEEN], RS_VERSION);
        assert_eq!(file["version_seen"], APP_VERSION, "the oracle's record stays as the Python Grainy keeps it");
        cx.close_dialog("whats_new");
        assert!(
            !settings::whats_new_on_start(&mut cx, false, true),
            "the version is remembered: nothing the second time"
        );
    }
    // From an older Grainy RS: the versions since, newest first.
    let (listed, mut cx) = start(&[(whats_new_box::SEEN, "0.3.0")]);
    assert_eq!(listed.first().map(String::as_str), Some(RS_VERSION));
    assert!(listed.len() > 1 && !listed.iter().any(|v| v == "0.3.0"), "{listed:?}");
    assert!(settings::whats_new_on_start(&mut cx, false, true));
    for seen in ["9.0.0", RS_VERSION] {
        let (listed, mut cx) = start(&[(whats_new_box::SEEN, seen)]);
        assert!(listed.is_empty(), "{seen}");
        assert!(!settings::whats_new_on_start(&mut cx, false, true), "{seen}");
    }
    let (_, mut cx) = start(&[]);
    assert!(!settings::whats_new_on_start(&mut cx, true, true), "a first run shows nothing");
    assert_eq!(stored(cx.data_folder().unwrap())[whats_new_box::SEEN], RS_VERSION, "but remembers the version");
    let (_, mut cx) = start(&[(whats_new_box::SEEN, "0.3.0")]);
    assert!(!settings::whats_new_on_start(&mut cx, false, false), "a program run from source shows nothing");
    assert_eq!(stored(cx.data_folder().unwrap())[whats_new_box::SEEN], "0.3.0");
}

/// `toggle_drt_advanced` stores the library preference `drt_advanced`, and a library opened later has the switch
/// as it was left.
#[test]
fn the_drt_switch_is_a_library_preference() {
    let g = Case::open("app", "settings_preferences").json().clone();
    let drt = &g["drt"];
    let folder = common::temp_dir("settings-drt").join("Library");
    let open = |folder: &std::path::Path| {
        let mut cx = context();
        cx.set_document(Box::new(CatalogDocument::open(folder).unwrap()));
        settings::poll(&mut cx);
        cx
    };
    let mut cx = open(&folder);
    assert_eq!(cx.drt_advanced(), drt["default"].as_bool().unwrap());
    for (value, key) in [(true, "after True"), (false, "after False"), (true, "after True")] {
        cx.set_drt_advanced(value);
        settings::poll(&mut cx);
        let stored = cx.doc.preference("drt_advanced").and_then(|v| v.as_bool());
        assert_eq!(stored, drt[key]["read"].as_bool(), "{key}");
        assert_eq!(drt[key]["stored"], if value { "true" } else { "false" });
    }
    drop(cx);
    assert!(open(&folder).drt_advanced(), "the switch is as it was left");
    // A session without a catalogue stores nothing.
    let mut session = context();
    session.set_drt_advanced(true);
    settings::poll(&mut session);
    assert!(session.doc.preference("drt_advanced").is_none());
}

// ---- the window state -----------------------------------------------------------------------------------------------

#[test]
fn the_window_state_round_trips_and_repairs_a_damaged_file() {
    let state = WindowState {
        size: Some([1700.0, 1010.0]),
        position: Some([40.0, 25.0]),
        maximized: true,
        full_screen: false,
        sides: false,
        bars: true,
        library_width: Some(300.0),
        develop_width: Some(320.0),
    };
    assert_eq!(WindowState::from_value(&state.to_value()), state);
    assert_eq!(WindowState::from_value(&json!(null)), WindowState::default());
    assert_eq!(WindowState::from_value(&json!([1, 2])), WindowState::default());
    // Sizes and widths come back inside the ranges the window allows; nonsense is left at the defaults.
    let damaged = WindowState::from_value(&json!({
        "size": [10, 20], "position": [1e9, 5], "maximized": "yes", "sides": 3, "library_width": 5000,
        "develop_width": "wide"}));
    assert_eq!(damaged.size, Some(window_state::MIN_SIZE));
    assert_eq!(damaged.position, None);
    assert!(!damaged.maximized && damaged.sides && damaged.bars);
    assert_eq!(damaged.library_width, Some(window_state::LIBRARY_RANGE.1));
    assert_eq!(damaged.develop_width, None);
    assert_eq!(WindowState::from_value(&json!({"size": [1, 2, 3]})).size, None);
    // The window opens with the stored size, position and mode.
    let builder = state.apply(egui::ViewportBuilder::default());
    assert_eq!(builder.inner_size, Some(egui::vec2(1700.0, 1010.0)));
    assert_eq!(builder.position, Some(egui::pos2(40.0, 25.0)));
    assert_eq!((builder.maximized, builder.fullscreen), (Some(true), Some(false)));
}

/// The state is written once it stayed the same for a while, never into the Python Grainy's folders, and read
/// back at start: the screen modes and the column widths.
#[test]
fn the_window_state_is_stored_and_restored() {
    let (mut cx, folder) = with_data("settings-window");
    assert!(window_state::load(&folder).is_none(), "nothing is stored before the window changed");
    window_state::poll(&mut cx);
    assert!(!folder.join("settings.json").exists(), "a change is written only after it stayed for a moment");
    std::thread::sleep(window_state::DELAY + Duration::from_millis(100));
    window_state::poll(&mut cx);
    assert!(folder.join("settings.json").exists());
    assert!(window_state::load(&folder).unwrap().sides);
    // The user hides the side columns, drags a column, goes full screen.
    assert!(cx.run("view.sides"));
    window_state::note_widths(&mut cx, Some(310.0), Some(333.4));
    window_state::flush(&mut cx);
    let value = &stored(&folder)["window"];
    assert_eq!(value["sides"], false);
    assert_eq!(value["library_width"], 310);
    assert_eq!(value["develop_width"], 333);
    // The next start has them again, and the other keys of the file stay.
    Preferences::new(folder.join("settings.json")).save_language("en").unwrap();
    let mut next = context();
    next.set_data_folder(Some(folder.clone()));
    window_state::restore(&mut next);
    assert!(!next.sides_visible() && next.bars_visible());
    assert_eq!(window_state::library_width(&mut next), 310.0);
    assert_eq!(window_state::develop_width(&mut next), 333.0);
    assert_eq!(stored(&folder)["language"], "en");
    // No folder, no file.
    let mut memory = context();
    window_state::flush(&mut memory);
    window_state::poll(&mut memory);
    assert_eq!(window_state::library_width(&mut memory), window_state::DEFAULT_LIBRARY);
    // The Python Grainy's own settings folder is never the data folder of the tests.
    let locations = grainy_app::paths::Locations::current();
    assert!(!grainy_app::paths::is_oracle_folder(&folder, &locations));
}

// ---- the window --------------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn empty_harness<'a>() -> Harness<'a, GrainyApp> {
    builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app
    })
}

fn photo_harness<'a>(paths: Vec<PathBuf>) -> Harness<'a, GrainyApp> {
    builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.open_paths(paths);
        app
    })
}

use common::snapshot::{save, settle};

fn both_languages(mut each: impl FnMut(Language, &str)) {
    let _turn = turn();
    for (language, suffix) in [(Language::Korean, "ko"), (Language::English, "en")] {
        grainy_i18n::set_language(language);
        each(language, suffix);
    }
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn settings_empty_view_asks_for_a_photo() {
    both_languages(|_, suffix| {
        let mut harness = empty_harness();
        harness.run_steps(4);
        save(&mut harness, &format!("settings-empty-{suffix}"));
    });
}

#[test]
fn settings_language_dialog() {
    both_languages(|_, suffix| {
        let mut harness = empty_harness();
        assert!(harness.state_mut().run("settings.language"));
        harness.run_steps(4);
        assert!(harness.state().dialog_open("language"));
        save(&mut harness, &format!("settings-language-{suffix}"));
        // The first-run question, with the combo box opened on the other language, is the same box.
        harness.state_mut().close_dialog("language");
        harness.state_mut().open_dialog(Box::new(grainy_app::dialogs::language::LanguageDialog::new(None, true, true)));
        harness.run_steps(4);
        save(&mut harness, &format!("settings-first-run-{suffix}"));
    });
}

#[test]
fn settings_whats_new_and_about() {
    both_languages(|_, suffix| {
        let mut harness = empty_harness();
        assert!(harness.state_mut().run("settings.whats_new"));
        harness.run_steps(4);
        save(&mut harness, &format!("settings-whats-new-{suffix}"));
        harness.state_mut().close_dialog("whats_new");
        assert!(harness.state_mut().run("settings.about"));
        harness.run_steps(4);
        save(&mut harness, &format!("settings-about-{suffix}"));
    });
}

#[test]
fn settings_update_offer() {
    both_languages(|_, suffix| {
        let mut harness = empty_harness();
        let g = Case::open("app", "settings_updates").json().clone();
        let release = release_of(&g["release"]);
        harness.state_mut().state::<UpdateState>().packaged = Some(false);
        settings::checked(harness.state_mut(), true, Ok(Some(release)));
        harness.run_steps(4);
        assert!(harness.state().dialog_open("update_offer"));
        save(&mut harness, &format!("settings-update-{suffix}"));
    });
}

/// The 설정 menu opened in the real window.
#[test]
fn settings_menu_is_drawn() {
    both_languages(|_, suffix| {
        let mut harness = empty_harness();
        harness.run_steps(3);
        harness.get_by_label(&grainy_i18n::tr("설정")).click();
        harness.run_steps(4);
        save(&mut harness, &format!("settings-menu-{suffix}"));
    });
}

/// L: only the photo; Esc brings everything back.
#[test]
fn settings_lights_out_shows_only_the_photo() {
    let _turn = turn();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("settings-lights");
    let path = dir.join("garden-large.png");
    common::save_png(&common::garden(6, [1.0; 3], false), &path);
    let mut harness = photo_harness(vec![path]);
    settle(&mut harness);
    harness.key_press(egui::Key::L);
    settle(&mut harness);
    assert!(settings::lights_on(harness.state()));
    save(&mut harness, "settings-lights-ko");
    harness.key_press(egui::Key::Escape);
    settle(&mut harness);
    assert!(!settings::lights_on(harness.state()), "Esc leaves lights out");
    assert!(harness.state().sides_visible() && harness.state().bars_visible(), "everything comes back");
}
