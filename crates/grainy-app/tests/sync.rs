//! Work package 12: sync and copy settings, against the oracle (`golden/app/sync`, built by
//! `tools/golden/gen_app_sync.py` with the Python Grainy's own `paste_edits`, `choose_copy`, `sync_selected`,
//! `after_commit`, `previous_edits` and `reset_selected` run on a stand-in for the window).
//!
//! The model tests drive the context without a window (what the window runs every frame after it stored the edits
//! is `commands::sync::after_flush`); the window tests render the real application headless and write
//! `tests/snapshots/sync-*`. Every test works on copies and generated photos in temporary folders.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::sync::{self, SyncState};
use grainy_app::commands::Invocation;
use grainy_app::dialogs::copy_items::{keys_of, CopyItems, EDIT_GROUPS};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::document::Document;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::{AppContext, GrainyApp, Mode};
use grainy_catalog::rusqlite::Connection;
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

fn case() -> Value {
    Case::open("app", "sync").json().clone()
}

// ---- the oracle's library -----------------------------------------------------------------------------------

fn golden_library(name: &str) -> (PathBuf, AppContext) {
    let meta = case();
    let dir = common::temp_dir(name);
    std::fs::copy(grainy_testkit::root().join("golden/app/sync/start.sqlite"), dir.join("catalog.sqlite"))
        .expect("copy the library");
    let stamp = meta["stamp"].as_str().unwrap().to_owned();
    let token = meta["token"].as_str().unwrap().to_owned();
    let catalog = Catalog::with_sources(&dir, Sources::new(move || stamp.clone(), move || token.clone())).unwrap();
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    (dir.clone(), context(doc))
}

fn context(doc: impl Document + 'static) -> AppContext {
    AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc))
}

/// The settings as the generator stores them: the keys that differ from the defaults, and how many keys.
fn sparse(settings: &Value) -> Value {
    let defaults = grainy_core::defaults();
    let map = settings.as_object().expect("settings are an object");
    let changed: Map<String, Value> = map
        .iter()
        .filter(|(k, v)| defaults.0.get(*k).is_none_or(|d| !grainy_core::settings::equivalent(v, d)))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    json!({"keys": map.len(), "changed": changed})
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// The catalogue rows the actions write (the generator's `project`).
fn project(path: &Path) -> Value {
    let db = Connection::open(path).unwrap();
    let mut photos = Map::new();
    let mut statement = db.prepare("SELECT id,edits,extras FROM photos ORDER BY id").unwrap();
    let rows = statement
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?)))
        .unwrap();
    for row in rows {
        let (id, edits, extras) = row.unwrap();
        let extras = extras.filter(|e| !e.is_empty()).map_or(json!({}), |e| parse(&e));
        photos.insert(id.to_string(), json!({"settings": sparse(&parse(&edits)), "extras": extras}));
    }
    let mut statement = db.prepare("SELECT id,photo_id,label,edits,created FROM history ORDER BY id").unwrap();
    let history: Vec<Value> = statement
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, i64>(0)?, "photo": r.get::<_, i64>(1)?, "label": r.get::<_, String>(2)?,
                      "settings": sparse(&parse(&r.get::<_, String>(3)?)), "created": r.get::<_, String>(4)?}))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut preferences = Map::new();
    let mut statement = db.prepare("SELECT key,value FROM preferences ORDER BY key").unwrap();
    let rows = statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).unwrap();
    for row in rows {
        let (key, text) = row.unwrap();
        let mut value = parse(&text);
        if key.starts_with("undo:") && value.get("undo").is_some() {
            let list = |name: &str| -> Value {
                Value::Array(value[name].as_array().map(|a| a.iter().map(sparse).collect()).unwrap_or_default())
            };
            let mut shown = Map::new();
            shown.insert("undo".into(), list("undo"));
            shown.insert("redo".into(), list("redo"));
            for (k, v) in value.as_object().unwrap() {
                if k != "undo" && k != "redo" {
                    shown.insert(k.clone(), v.clone());
                }
            }
            value = Value::Object(shown);
        }
        preferences.insert(key, value);
    }
    json!({"photos": photos, "history": history, "preferences": preferences})
}

fn equal(actual: &Value, expected: &Value, at: &str) {
    if actual == expected {
        return;
    }
    match (actual, expected) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                equal(a.get(k).unwrap_or(&Value::Null), v, &format!("{at}.{k}"));
            }
            assert_eq!(
                a.len(),
                b.len(),
                "{at}: keys {:?} vs {:?}",
                a.keys().collect::<Vec<_>>(),
                b.keys().collect::<Vec<_>>()
            );
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                equal(x, y, &format!("{at}[{i}]"));
            }
            assert_eq!(a.len(), b.len(), "{at}: length");
        }
        _ => panic!("{at}\n  rust:   {actual}\n  oracle: {expected}"),
    }
}

// ---- the scripted steps ---------------------------------------------------------------------------------------

/// What the window does every frame after it changed something: the edits are stored, then Auto Sync and the
/// previous photo are looked at.
fn frame(cx: &mut AppContext) {
    cx.doc.flush().unwrap();
    sync::after_flush(cx);
}

fn open(cx: &mut AppContext, id: u64) {
    let index = cx.doc.index_of(id).unwrap_or_else(|| panic!("photo {id} is listed"));
    cx.select_photo(index);
}

fn select_ids(cx: &mut AppContext, ids: &[u64]) {
    let current = cx.doc.current_index().unwrap();
    cx.doc.select(current);
    for id in ids {
        if *id != cx.doc.current_id() {
            let index = cx.doc.index_of(*id).unwrap();
            cx.doc.toggle_selected(index);
        }
    }
}

/// The dialog's answer: the titles ticked, every other group cleared.
fn choose(cx: &mut AppContext, ticked: &[&str]) {
    let keys = cx.state::<SyncState>().copy_keys.clone();
    let mut dialog = CopyItems::new(&keys);
    for (title, _) in EDIT_GROUPS {
        dialog.set(title, ticked.contains(&title));
    }
    let checks = dialog.checks().to_vec();
    sync::choose_done(cx, &checks);
}

fn run_step(cx: &mut AppContext, step: &Value) {
    let a = step.as_array().unwrap();
    match a[0].as_str().unwrap() {
        "open" => open(cx, a[1].as_u64().unwrap()),
        "set" => cx.doc.edit_mut().set(a[1].as_str().unwrap(), a[2].clone()),
        "commit" => {
            cx.doc.edit_mut().commit(a.get(1).and_then(Value::as_str).unwrap_or("보정 변경"));
        }
        "select" => {
            let ids: Vec<u64> = a[1].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
            select_ids(cx, &ids);
        }
        "copy" => assert!(cx.run("develop.copy_edits")),
        "paste" => assert!(cx.run("develop.paste_edits")),
        "choose" => {
            let titles: Vec<String> = a[1].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
            let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
            choose(cx, &titles);
        }
        "copy_crop" => assert!(cx.run_with("sync.copy_crop", &Invocation::value(a[1].as_bool().unwrap()))),
        "sync" => assert!(cx.run("sync.selected")),
        "auto" => assert!(cx.run_with("sync.auto", &Invocation::value(a[1].as_bool().unwrap()))),
        "previous" => assert!(cx.run("develop.paste_previous")),
        "reset_selected" => assert!(cx.run("sync.reset_selected")),
        "undo" => assert!(cx.run("develop.undo")),
        "redo" => assert!(cx.run("develop.redo")),
        other => panic!("unknown step {other}"),
    }
    frame(cx);
}

/// A scenario of the generator through the Rust commands: after every step the messages, the copy set and the
/// previous photo equal the oracle's; at every checkpoint (and at the end) the catalogue equals the oracle's.
// The Python fixtures predate the RS denoiser and the process-version group choices.
fn rs_copy_keys(legacy: &Value) -> Vec<String> {
    let mut keys: std::collections::BTreeSet<String> = serde_json::from_value(legacy.clone()).unwrap();
    if keys.contains("sharpen") {
        keys.extend(["ai_denoise".to_owned(), "detail_version".to_owned()]);
    }
    if keys.contains("exposure") {
        keys.insert("tone_version".to_owned());
    }
    keys.into_iter().collect()
}

fn run_session(name: &str) {
    // The interface language is process-wide and the status lines are compared in Korean: not while a window test
    // renders in English.
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let meta = case();
    let session = &meta["sessions"][name];
    let (dir, mut cx) = golden_library(&format!("sync-{name}"));
    let path = dir.join("catalog.sqlite");
    let mut steps = session["steps"].as_array().unwrap().iter();
    for step in session["script"].as_array().unwrap() {
        let a = step.as_array().unwrap();
        if a[0] == "check" {
            cx.doc.edit_mut().commit("보정 변경");
            frame(&mut cx);
            let label = a[1].as_str().unwrap();
            equal(&project(&path), &session["checkpoints"][label], label);
            println!("checkpoint {label}: the catalogue equals the oracle's");
            continue;
        }
        let expected = steps.next().expect("one record per step");
        assert_eq!(&expected["step"], step);
        cx.set_status("");
        run_step(&mut cx, step);
        let at = format!("{name} {step}");
        if let Some(message) = expected["messages"].as_array().and_then(|m| m.last()).and_then(Value::as_str) {
            assert_eq!(cx.status(), message, "{at}: the status line");
        }
        if let Some(error) = expected["error"].as_str() {
            assert_eq!(cx.status(), format!("처리하지 못했습니다: {error}"), "{at}: the error");
        }
        let state = cx.state::<SyncState>().clone();
        let keys: Vec<String> = state.copy_keys.iter().cloned().collect();
        assert_eq!(keys, rs_copy_keys(&expected["copy_keys"]), "{at}: the copy set");
        assert_eq!(state.copy_crop, expected["copy_crop"].as_bool().unwrap(), "{at}: crop and rotation");
        assert_eq!(state.previous, expected["previous"].as_u64(), "{at}: the previous photo");
    }
    cx.doc.edit_mut().commit("보정 변경");
    frame(&mut cx);
    equal(&project(&path), &session["final"], "final");
}

#[test]
fn the_scripted_session_writes_what_the_oracle_writes() {
    run_session("main");
}

#[test]
fn the_previous_photo_is_unknown_until_another_photo_was_open() {
    run_session("no_previous");
}

#[test]
fn the_copy_groups_and_the_default_copy_set_are_the_oracle_s() {
    let _turn = common::language_lock();
    let meta = case();
    let groups = meta["groups"].as_object().unwrap();
    assert_eq!(groups.len(), EDIT_GROUPS.len());
    for (title, keys) in EDIT_GROUPS {
        let mut expected: Vec<&str> = groups[title].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect();
        match title {
            "빛" => expected.insert(0, "tone_version"),
            "디테일" => {
                let index = expected.iter().position(|k| *k == "noise_luma").unwrap();
                expected.splice(index..index, ["detail_version", "ai_denoise"]);
            }
            _ => {}
        }
        assert_eq!(keys, expected.as_slice(), "{title}");
    }
    // The dialog lists the groups in the oracle's order (`EDIT_GROUPS` is a dict in the oracle's source order).
    let titles: Vec<&str> = EDIT_GROUPS.iter().map(|(t, _)| *t).collect();
    assert_eq!(
        titles,
        [
            "작업 색공간",
            "빛",
            "HDR",
            "화이트 밸런스",
            "RAW 프로파일",
            "색상",
            "필름",
            "컬러 필터",
            "커브",
            "디테일",
            "효과",
            "네거티브 반전",
            "렌즈·원근",
            "크롭·회전",
            "마스크",
            "복구"
        ]
    );
    let default = rs_copy_keys(&meta["default_copy_keys"]);
    let state = SyncState::default();
    assert_eq!(state.copy_keys.iter().cloned().collect::<Vec<_>>(), default, "everything but crop and rotation");
    assert!(!state.copy_crop && !state.auto && state.previous.is_none());
    // Every group of the default set shows ticked, the crop group not.
    let dialog = CopyItems::new(&state.copy_keys);
    for (title, on) in dialog.checks() {
        assert_eq!(*on, *title != "크롭·회전", "{title}");
    }
    let grouped: usize = EDIT_GROUPS.iter().filter(|(t, _)| *t != "크롭·회전").map(|(_, k)| k.len()).sum();
    assert_eq!(keys_of(dialog.checks()).len(), grouped, "the ticked groups: no crop keys");
}

// ---- documents without a catalogue and read-only libraries ------------------------------------------------

fn three_photos(name: &str) -> Vec<PathBuf> {
    let dir = common::temp_dir(name);
    common::eight_photos(&dir)[..3].to_vec()
}

#[test]
fn a_session_pastes_syncs_follows_and_resets() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let mut session = Session::default();
    session.add(three_photos("sync-session"));
    let mut cx = context(session);
    let setting = |cx: &AppContext, index: usize, key: &str| cx.doc.settings_of(index).f64(key);
    // The open photo is edited, three photos are selected, the current edits are synced.
    cx.doc.edit_mut().set("exposure", 0.7);
    cx.doc.edit_mut().set("contrast", 18.0);
    cx.doc.edit_mut().commit("보정 변경");
    frame(&mut cx);
    cx.doc.toggle_selected(1);
    cx.doc.toggle_selected(2);
    assert!(cx.run("sync.selected"));
    frame(&mut cx);
    assert_eq!(setting(&cx, 1, "exposure"), 0.7);
    assert_eq!(setting(&cx, 2, "contrast"), 18.0);
    assert_eq!(cx.status(), "2장에 선택한 보정 항목을 동기화했습니다.");
    // Two groups only.
    choose(&mut cx, &["빛"]);
    cx.doc.edit_mut().set("exposure", -0.4);
    cx.doc.edit_mut().set("saturation", 33.0);
    cx.doc.edit_mut().commit("보정 변경");
    assert!(cx.run("sync.selected"));
    frame(&mut cx);
    assert_eq!(setting(&cx, 1, "exposure"), -0.4);
    assert_eq!(setting(&cx, 1, "saturation"), 0.0, "colour is not in the ticked group");
    // Auto Sync: a commit follows the selection.
    assert!(cx.run_with("sync.auto", &Invocation::value(true)));
    cx.doc.edit_mut().set("exposure", 1.5);
    cx.doc.edit_mut().commit("보정 변경");
    frame(&mut cx);
    assert_eq!(setting(&cx, 2, "exposure"), 1.5, "Auto Sync");
    assert!(cx.run_with("sync.auto", &Invocation::value(false)));
    cx.doc.edit_mut().set("exposure", 2.0);
    cx.doc.edit_mut().commit("보정 변경");
    frame(&mut cx);
    assert_eq!(setting(&cx, 2, "exposure"), 1.5, "Auto Sync off");
    // The previous photo.
    cx.select_photo(1);
    frame(&mut cx);
    assert_eq!(cx.state::<SyncState>().previous, Some(1));
    cx.doc.toggle_selected(2);
    assert!(cx.run("develop.paste_previous"));
    frame(&mut cx);
    assert_eq!(setting(&cx, 1, "exposure"), 2.0, "the photo left last, as the oracle remembers it");
    // Reset.
    assert!(cx.run("sync.reset_selected"));
    frame(&mut cx);
    assert_eq!(setting(&cx, 1, "exposure"), 0.0);
    assert_eq!(setting(&cx, 2, "exposure"), 0.0);
    assert_eq!(cx.status(), "2장의 사진에 적용했습니다. 사진별 실행 취소로 되돌릴 수 있습니다.");
}

#[test]
fn a_read_only_library_syncs_in_memory_only() {
    let _turn = common::language_lock();
    let dir = common::temp_dir("sync-read-only");
    let library = dir.join("Library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::copy(grainy_testkit::root().join("golden/app/sync/start.sqlite"), library.join("catalog.sqlite")).unwrap();
    let original = std::fs::read(library.join("catalog.sqlite")).unwrap();
    let mut doc = CatalogDocument::open_read_only(&library, &dir.join("scratch")).unwrap();
    doc.refresh_now().unwrap();
    let mut cx = context(doc);
    open(&mut cx, 1);
    select_ids(&mut cx, &[1, 2, 3]);
    assert!(cx.run("sync.selected"));
    frame(&mut cx);
    assert_eq!(cx.doc.settings_of(1).f64("exposure"), cx.doc.settings_of(0).f64("exposure"));
    assert!(cx.run("sync.reset_selected"));
    frame(&mut cx);
    assert_eq!(cx.doc.settings_of(2).f64("exposure"), 0.0, "reset in memory");
    drop(cx);
    assert!(std::fs::read(library.join("catalog.sqlite")).unwrap() == original, "the catalogue file is unchanged");
}

// ---- windows -----------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle(harness);
}

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) {
    common::snapshot::save(harness, name);
}

/// The box 복사·동기화 항목 with the crop group ticked and two groups cleared.
fn dialog_snapshot(language: Language, name: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let mut harness = builder().wgpu().build_eframe(|cc| {
        let mut app = GrainyApp::with_test_card(&cc.egui_ctx);
        let state = app.state::<SyncState>();
        state.set_copy_crop(true);
        for title in ["필름", "효과"] {
            for key in grainy_app::dialogs::copy_items::group(title) {
                state.copy_keys.remove(*key);
            }
        }
        assert!(app.run("sync.choose_items"));
        app
    });
    harness.run_steps(4);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_copy_items_box_korean() {
    dialog_snapshot(Language::Korean, "sync-dialog-ko");
}

#[test]
fn the_copy_items_box_english() {
    dialog_snapshot(Language::English, "sync-dialog-en");
}

/// A library of eight photos: the open photo graded, then synced to the selected ones; the grid shows the
/// synced thumbnails and the status line the message.
fn synced_snapshot(language: Language, name: &str, scene: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let dir = common::temp_dir(scene);
    std::fs::create_dir_all(dir.join("source")).unwrap();
    let photos = common::eight_photos(&dir.join("source"));
    let library = dir.join("Library");
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let doc = CatalogDocument::open(&library).expect("library");
        let mut app =
            GrainyApp::with_document(&cc.egui_ctx, Box::new(grainy_app::worker::Worker::new()), Box::new(doc));
        app.open_paths(photos);
        app.set_mode(Mode::Library);
        app
    });
    settle(&mut harness);
    assert_eq!(harness.state().document().len(), 8, "{}", harness.state().status());
    {
        let app = harness.state_mut();
        let edit = app.edit_state_mut();
        edit.set("exposure", 0.9);
        edit.set("monochrome", true);
        edit.set("contrast", 35.0);
        edit.commit("보정 변경");
        for index in [2, 3, 5] {
            app.document_mut().toggle_selected(index);
        }
        assert!(app.run("sync.selected"));
    }
    settle(&mut harness);
    // The thumbnails are developed; the message is on the status line again (a second sync writes nothing).
    assert!(harness.state_mut().run("sync.selected"));
    harness.run_steps(2);
    assert_eq!(harness.state().status(), grainy_i18n::tr_with("{0}장에 선택한 보정 항목을 동기화했습니다.", &["3"]));
    common::snapshot::open_folders(harness.state_mut(), &[]);
    harness.run_steps(3);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn synced_photos_korean() {
    synced_snapshot(Language::Korean, "sync-synced-ko", "sync-synced-ko");
}

#[test]
fn synced_photos_english() {
    synced_snapshot(Language::English, "sync-synced-en", "sync-synced-en");
}
