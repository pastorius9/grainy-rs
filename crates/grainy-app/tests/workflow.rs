//! Work package 18: the batch workflow of the 사진 작업 menu, against the oracle (`golden/app/workflow`, built by
//! `tools/golden/gen_app_workflow.py` with the Python Grainy's own `WorkflowTools` methods on a stand-in for the
//! window, over real generated photos: automatic tone, colour and white balance, negatives and rolls, the scan-border
//! crop, the process update, exposure matching, the metadata presets and the shortcut keyword).
//!
//! The model tests drive the context without a window: the same scripted steps through the Rust commands (jobs
//! included, waited for like the window does every frame) and, after each step, the status line and the message
//! boxes equal the oracle's; at every checkpoint every table of the catalogue (photos with their settings, extras
//! and keywords, history rows, preferences with the undo stacks) equals the oracle's. Numbers are compared to
//! `1e-4` (relative; the engine's float32 maths ports are tested to that against the oracle) and the largest
//! difference found is printed. The window tests render the boxes headless (snapshots in Korean and English).
//!
//! Every test works on temporary folders and the generated photos of the golden; no library of the owner is opened.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::library_column::LAUNCH_DISABLED;
use grainy_app::commands::workflow::{
    choose_done, keyword_done, preset_names, save_preset_done, saved_keyword, Choice, Then, WorkflowState, CANCEL_NEXT,
    FIELDS,
};
use grainy_app::dialogs::metadata_preset::{KeywordInput, PresetChoose, PresetSave};
use grainy_app::dialogs::progress::{ProgressBox, Shared};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::document::Document;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::{AppContext, GrainyApp};
use grainy_catalog::rusqlite::Connection;
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

fn case() -> Value {
    Case::open("app", "workflow").json().clone()
}

// ---- the oracle's library -------------------------------------------------------------------------------------

/// The library of the generator in a temporary folder: the golden's generated photos (and `gone.png`, a photo whose
/// file was removed after it was added) with the oracle's settings, metadata, extras and keywords.
fn golden_library(name: &str) -> (PathBuf, AppContext) {
    let meta = case();
    let dir = common::temp_dir(name);
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos).unwrap();
    let golden = grainy_testkit::root().join("golden/app/workflow/photos");
    let stamp = meta["stamp"].as_str().unwrap().to_owned();
    let token = meta["token"].as_str().unwrap().to_owned();
    let catalog =
        Catalog::with_sources(dir.join("library"), Sources::new(move || stamp.clone(), move || token.clone())).unwrap();
    for entry in meta["library"].as_array().unwrap() {
        let file = entry["name"].as_str().unwrap();
        // The photo whose file is gone is the plain one, removed once it is in the library.
        let source = if entry["file"].as_bool().unwrap() { file } else { "plain.png" };
        std::fs::copy(golden.join(source), photos.join(file)).unwrap();
        let settings = common::settings(entry["settings"].clone());
        let id =
            catalog.add(&photos.join(file).to_string_lossy(), Some(&settings), Some(&entry["info"]), false).unwrap();
        let mut fields = Map::new();
        fields.insert("extras".into(), Value::String(entry["extras"].to_string()));
        fields.insert("keywords".into(), entry["keywords"].clone());
        catalog.update(id, &fields).unwrap();
        if !entry["file"].as_bool().unwrap() {
            std::fs::remove_file(photos.join(file)).unwrap();
        }
    }
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    (dir.clone(), context(doc))
}

fn context(doc: impl Document + 'static) -> AppContext {
    AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc))
}

fn database(dir: &Path) -> PathBuf {
    dir.join("library").join("catalog.sqlite")
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
    let mut statement = db.prepare("SELECT id,edits,extras,keywords FROM photos ORDER BY id").unwrap();
    let rows = statement
        .query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?))
        })
        .unwrap();
    for row in rows {
        let (id, edits, extras, keywords) = row.unwrap();
        let extras = extras.filter(|e| !e.is_empty()).map_or(json!({}), |e| parse(&e));
        photos.insert(
            id.to_string(),
            json!({"settings": sparse(&parse(&edits)), "extras": extras, "keywords": keywords}),
        );
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

/// The largest numeric difference `equal` has seen.
static LARGEST: Mutex<f64> = Mutex::new(0.0);

/// Deep equality; numbers within `1e-4` relative (the float32 stages of the engine differ from numpy's in the last
/// bits), everything else exact.
fn equal(actual: &Value, expected: &Value, at: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            let difference = (a - b).abs() / (1.0 + b.abs());
            if let Ok(mut largest) = LARGEST.lock() {
                *largest = largest.max(difference);
            }
            assert!(difference <= 1e-4, "{at}: {a} against the oracle's {b}");
        }
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
        _ => assert_eq!(actual, expected, "{at}"),
    }
}

// ---- the scripted steps -------------------------------------------------------------------------------------------

fn frame(cx: &mut AppContext) {
    cx.doc.flush().unwrap();
}

fn open(cx: &mut AppContext, id: u64) {
    let index = cx.doc.index_of(id).unwrap_or_else(|| panic!("photo {id} is listed"));
    cx.select_photo(index);
}

/// The first photo of the list is opened, the others join the selection (the window's selection holds the open photo).
fn select_ids(cx: &mut AppContext, ids: &[u64]) {
    open(cx, ids[0]);
    for id in &ids[1..] {
        let index = cx.doc.index_of(*id).unwrap_or_else(|| panic!("photo {id} is listed"));
        cx.doc.toggle_selected(index);
    }
}

/// Runs the finished jobs of the context until no batch is left (what the window does every frame).
fn wait(cx: &mut AppContext) {
    let started = Instant::now();
    while cx.jobs.running("workflow") {
        for finished in cx.jobs.poll() {
            finished(cx);
        }
        assert!(started.elapsed() < Duration::from_secs(120), "the batch did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    for finished in cx.jobs.poll() {
        finished(cx);
    }
    for id in ["workflow_progress", "workflow_error"] {
        cx.close_dialog(id);
    }
}

fn texts(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect()
}

/// The rows of a save form as the generator records them.
fn rows_of(dialog: &PresetSave) -> Value {
    json!(dialog.rows.iter().map(|r| json!([r.title, r.checked, r.text])).collect::<Vec<_>>())
}

fn shown<'a>(expected: &'a Value, key: &str) -> Option<&'a Value> {
    expected["shown"].as_array().unwrap().iter().find_map(|s| s.get(key))
}

fn run_step(cx: &mut AppContext, step: &Value, expected: &Value, at: &str) {
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
        "pref" => cx.doc.save_preference(a[1].as_str().unwrap(), &a[2]).unwrap(),
        "cancel_next" => CANCEL_NEXT.store(true, Ordering::SeqCst),
        "auto_tone" | "auto_color" | "auto_white" | "invert_negatives" | "invert_roll" | "border_crop" => {
            assert!(cx.run(&format!("workflow.{}", a[0].as_str().unwrap())), "{at}: the command is enabled");
            // A batch that has something to do opens the progress box and runs a job.
            wait(cx);
        }
        "update_process" | "match_exposure" => {
            assert!(cx.run(&format!("workflow.{}", a[0].as_str().unwrap())), "{at}: the command is enabled");
        }
        "save_metadata" => {
            let id = cx.doc.current_id() as i64;
            let row = cx.doc.metadata_catalog().unwrap().photo(id).unwrap().unwrap();
            assert!(cx.run("workflow.metadata_save"), "{at}");
            assert!(cx.dialog_open("workflow_preset_save"), "{at}: the form opens");
            cx.close_dialog("workflow_preset_save");
            let mut dialog = PresetSave::new(&row);
            let recorded = &expected["shown"][0];
            assert_eq!(dialog.name, recorded["name"].as_str().unwrap(), "{at}: the name field");
            assert_eq!(rows_of(&dialog), recorded["rows"], "{at}: what the form shows");
            assert_eq!(grainy_app::text::tr("메타데이터 프리셋 저장"), recorded["title"].as_str().unwrap());
            if let Some(answer) = a.get(1).filter(|v| v.is_object()) {
                dialog.name = answer["name"].as_str().unwrap_or("").to_owned();
                for row in &mut dialog.rows {
                    if let Some(ticked) = answer.get("ticked").filter(|t| !t.is_null()) {
                        row.checked = texts(ticked).iter().any(|t| t == row.title);
                    }
                    if let Some(text) = answer.get("texts").and_then(|t| t.get(row.title)) {
                        row.text = text.as_str().unwrap().to_owned();
                    }
                }
                save_preset_done(cx, &dialog.name, &dialog.rows);
            }
        }
        name @ ("apply_metadata" | "delete_metadata") => {
            let what = if name == "apply_metadata" { Choice::Apply } else { Choice::Delete };
            let before = preset_names(cx);
            assert!(cx.run(&format!("workflow.metadata_{}", name.trim_end_matches("_metadata"))), "{at}");
            let list = shown(expected, "item_box");
            assert_eq!(cx.dialog_open("workflow_preset_choose"), list.is_some(), "{at}: the list opens");
            if let Some(list) = list {
                assert_eq!(json!(before), list[2], "{at}: the saved presets");
                assert_eq!(grainy_app::text::tr("메타데이터 프리셋"), list[0].as_str().unwrap());
                assert_eq!(grainy_app::text::tr("프리셋 선택"), list[1].as_str().unwrap());
                cx.close_dialog("workflow_preset_choose");
                if let Some(chosen) = a[1].as_str() {
                    choose_done(cx, what, chosen);
                }
            }
        }
        name @ ("set_keyword" | "apply_keyword") => {
            let before = saved_keyword(cx);
            let (id, then) = if name == "set_keyword" {
                ("workflow.keyword_set", Then::Set)
            } else {
                ("workflow.keyword_apply", Then::Apply)
            };
            assert!(cx.run(id), "{at}");
            let asked = shown(expected, "text_box");
            assert_eq!(cx.dialog_open("workflow_keyword"), asked.is_some(), "{at}: the box opens");
            if let Some(asked) = asked {
                assert_eq!(json!(before), asked[2], "{at}: the saved keyword in the box");
                assert_eq!(grainy_app::text::tr("바로가기 키워드"), asked[0].as_str().unwrap());
                assert_eq!(grainy_app::text::tr("키워드 하나 · 계층은 |로 구분"), asked[1].as_str().unwrap());
                cx.close_dialog("workflow_keyword");
                if let Some(text) = a.get(1).and_then(Value::as_str) {
                    keyword_done(cx, then, text);
                }
            }
        }
        other => panic!("unknown step {other}"),
    }
    frame(cx);
}

/// A scenario of the generator through the Rust commands: after every step the status line and the message boxes
/// equal the oracle's; at every checkpoint the catalogue equals the oracle's.
fn run_session(name: &str) {
    // The interface language is process-wide and the status lines are compared in Korean: not while a window test
    // renders in English.
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let meta = case();
    let session = &meta["sessions"][name];
    let (dir, mut cx) = golden_library(&format!("workflow-{name}"));
    let path = database(&dir);
    equal(&project(&path), &meta["start"], "start");
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
        let before = cx.state::<WorkflowState>().errors.len();
        let at = format!("{name} {step}");
        run_step(&mut cx, step, expected, &at);
        let boxes = cx.state::<WorkflowState>().errors[before..].to_vec();
        let wanted = texts(&expected["boxes"]);
        // A load failure names a file of the temporary folder: the generator does not store the system's text.
        let same = boxes.len() == wanted.len()
            && boxes.iter().zip(&wanted).all(|(b, w)| b == w || (w == "<load error>" && b.contains("Error")));
        assert!(same, "{at}: the message boxes {boxes:?} against the oracle's {wanted:?}");
        if let Some(message) = expected["messages"].as_array().and_then(|m| m.last()).and_then(Value::as_str) {
            assert_eq!(cx.status(), message, "{at}: the status line");
        }
    }
    println!("{name}: largest numeric difference {:e}", LARGEST.lock().map(|l| *l).unwrap_or(0.0));
}

#[test]
fn the_library_starts_as_the_oracles() {
    let _turn = common::language_lock();
    let meta = case();
    let (dir, _cx) = golden_library("workflow-start");
    equal(&project(&database(&dir)), &meta["start"], "start");
}

#[test]
fn automatic_tone_colour_and_white_balance_for_a_selection() {
    run_session("batch");
}

#[test]
fn negatives_and_rolls() {
    run_session("negatives");
}

#[test]
fn the_scan_border_crop() {
    run_session("border");
}

#[test]
fn the_process_update() {
    run_session("process");
}

#[test]
fn exposure_matching() {
    run_session("exposure");
}

#[test]
fn the_metadata_presets() {
    run_session("metadata");
}

#[test]
fn the_shortcut_keyword() {
    run_session("keywords");
}

#[test]
fn the_form_lists_the_oracles_fields_in_its_order() {
    let meta = case();
    let titles: Vec<&str> = FIELDS.iter().map(|(_, t)| *t).collect();
    assert_eq!(json!(titles), meta["form_fields"]);
    let keys: Vec<&str> = FIELDS.iter().map(|(k, _)| *k).collect();
    assert_eq!(
        keys,
        [
            "title",
            "caption",
            "creator",
            "copyright",
            "location",
            "city",
            "country",
            "latitude",
            "longitude",
            "DateTimeOriginal",
            "keywords"
        ]
    );
}

// ---- cancel and failure leave the catalogue as it was --------------------------------------------------------------

#[test]
fn a_cancelled_or_failed_batch_changes_nothing() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let (dir, mut cx) = golden_library("workflow-nothing");
    let path = database(&dir);
    let before = project(&path);
    // Cancelled before the first photo.
    select_ids(&mut cx, &[4, 5]);
    CANCEL_NEXT.store(true, Ordering::SeqCst);
    assert!(cx.run("workflow.auto_tone"));
    wait(&mut cx);
    frame(&mut cx);
    equal(&project(&path), &before, "cancelled");
    // One photo cannot be read (its file and its offline preview are gone).
    select_ids(&mut cx, &[4, 9]);
    assert!(cx.run("workflow.auto_color"));
    wait(&mut cx);
    frame(&mut cx);
    equal(&project(&path), &before, "unreadable");
    let errors = cx.state::<WorkflowState>().errors.clone();
    assert_eq!(
        errors,
        [
            "일괄 자동톤을 취소했습니다. 변경한 사진은 없습니다.",
            "일부 사진을 읽거나 계산할 수 없습니다. 변경한 사진은 없습니다."
        ]
    );
    // Nothing runs any more and the next batch can start.
    assert!(!cx.jobs.running("workflow"));
    select_ids(&mut cx, &[1, 2]);
    assert!(cx.run("workflow.auto_tone"));
    wait(&mut cx);
    frame(&mut cx);
    assert_ne!(project(&path), before, "the next batch writes");
}

#[test]
fn a_batch_edit_made_while_it_ran_is_refused() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let (dir, mut cx) = golden_library("workflow-conflict");
    let path = database(&dir);
    select_ids(&mut cx, &[1, 2]);
    // The rows the batch froze are not the catalogue's any more when its values arrive (another connection wrote).
    let records: Vec<Value> = [1i64, 2]
        .iter()
        .map(|id| {
            let row = cx.doc.metadata_catalog().unwrap().photo(*id).unwrap().unwrap();
            json!({"id": id, "path": row["path"], "settings": row["settings"]})
        })
        .collect();
    cx.doc.edit_mut().set("exposure", json!(1.5));
    cx.doc.edit_mut().commit("보정 변경");
    frame(&mut cx);
    let before = project(&path);
    let values = vec![common::settings(json!({"contrast": 30.0})), common::settings(json!({"contrast": 30.0}))];
    let error = grainy_app::commands::workflow::apply_edits(&mut cx, &records, values, "자동톤").unwrap_err();
    assert_eq!(error, "대상 사진 또는 보정값이 바뀌었습니다. 다시 요청해 주세요. 변경한 사진은 없습니다.");
    equal(&project(&path), &before, "refused");
}

// ---- a session without a catalogue ---------------------------------------------------------------------------------

#[test]
fn a_session_has_the_batches_too() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("workflow-session");
    let mut session = Session::default();
    session.add(common::eight_photos(&dir)[..3].to_vec());
    let mut cx = context(session);
    select_ids(&mut cx, &[1, 2, 3]);
    assert!(cx.run("workflow.update_process"));
    assert_eq!(cx.status(), grainy_app::text::tr("선택한 사진은 이미 최신 처리 방식입니다."));
    assert!(cx.run("workflow.auto_tone"));
    wait(&mut cx);
    frame(&mut cx);
    let tone = |cx: &AppContext, index: usize| cx.doc.settings_of(index).f64("exposure");
    // The garden at half and at twice the light: the automatic tone gives each its own exposure.
    assert!((tone(&cx, 0) - tone(&cx, 1)).abs() > 0.5, "{} {}", tone(&cx, 0), tone(&cx, 1));
    assert!(cx.status().starts_with("자동톤 · "), "{}", cx.status());
    assert!(cx.state::<WorkflowState>().errors.is_empty());
    // A preset needs a catalogue.
    assert!(!cx.run("workflow.metadata_save"));
}

// ---- the window ---------------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1280.0, 720.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The window on a new library in a temporary folder with two generated photos, the first open.
fn library_harness<'a>(name: &str) -> (Harness<'a, GrainyApp>, PathBuf) {
    let dir = common::temp_dir(name);
    let photos = [dir.join("garden.png"), dir.join("garden two.png")];
    common::save_png(&common::garden(2, [1.0; 3], false), &photos[0]);
    common::save_png(&common::garden(2, [0.8, 1.0, 1.1], true), &photos[1]);
    let library = dir.join("Library");
    let data = common::data_dir(&dir);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_library(&cc.egui_ctx, &library, Some(data));
        app.set_debug_overlay(false);
        app.open_paths(photos.to_vec());
        app
    });
    common::snapshot::settle(&mut harness);
    (harness, dir)
}

fn reader(dir: &Path) -> Catalog {
    Catalog::open_reader(dir.join("Library")).unwrap()
}

/// What a picture whose subject is a box needs fixed: the folder tree, the status line.
fn pin(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::open_folders(harness.state_mut(), &[]);
    common::snapshot::pin_status(harness.state_mut(), "");
    harness.run_steps(3);
}

fn flow(language: Language, tag: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    LAUNCH_DISABLED.store(true, Ordering::Relaxed);
    let (mut harness, dir) = library_harness(&format!("workflow-window-{tag}"));
    let ids: Vec<i64> = harness.state().cx.doc.photos().iter().map(|p| p.id as i64).collect();
    assert_eq!(ids.len(), 2);

    // 자동 톤 for both photos through the menu's command: the progress box, the job, one batch in the catalogue.
    {
        let cx = &mut harness.state_mut().cx;
        let index = cx.doc.index_of(ids[0] as u64).unwrap();
        cx.select_photo(index);
        cx.doc.toggle_selected(cx.doc.index_of(ids[1] as u64).unwrap());
        assert!(cx.run("workflow.auto_tone"));
        assert!(cx.dialog_open("workflow_progress"), "the progress box is on while the job runs");
        assert!(!commands_enabled(cx, "workflow.auto_tone"), "one batch at a time");
    }
    common::snapshot::settle(&mut harness);
    harness.run_steps(3);
    assert!(!harness.state().dialog_open("workflow_progress"), "the box closes with the job");
    {
        let catalog = reader(&dir);
        let rows = catalog.rows("SELECT photo_id,label FROM history ORDER BY id", []).unwrap();
        let labels: Vec<(i64, String)> =
            rows.iter().map(|r| (r["photo_id"].as_i64().unwrap(), r["label"].as_str().unwrap().to_owned())).collect();
        assert_eq!(labels, [(ids[0], "자동톤".to_owned()), (ids[1], "자동톤".to_owned())]);
        // Each photo has its own undo step.
        for id in &ids {
            let undo = catalog.preference(&format!("undo:{id}"), json!({})).unwrap();
            assert_eq!(undo["undo"].as_array().unwrap().len(), 1);
        }
    }
    // The open photo shows the new values (its state was read again).
    assert!(harness.state().cx.doc.edit().settings().f64("exposure") != 0.0);

    // The progress box over the library: three of eight photos done.
    let shared = Shared::new(8);
    shared.set_done(3);
    {
        let cx = &mut harness.state_mut().cx;
        cx.open_dialog(Box::new(ProgressBox::new(
            0,
            grainy_app::text::tr("일괄 자동톤"),
            "사진별 자동톤을 계산하는 중…".into(),
            shared.clone(),
        )));
    }
    pin(&mut harness);
    common::snapshot::save(&mut harness, &format!("workflow-progress-{tag}"));
    shared.finish();
    harness.run_steps(3);
    assert!(!harness.state().dialog_open("workflow_progress"));

    // The form that saves a metadata preset, for the open photo (with a creator and a copyright).
    {
        let cx = &mut harness.state_mut().cx;
        let id = cx.doc.current_id() as i64;
        let mut values = Map::new();
        values.insert("title".into(), json!("정원"));
        values.insert("creator".into(), json!("Sample Creator"));
        values.insert("copyright".into(), json!("2026 Sample"));
        cx.doc.metadata_catalog().unwrap().set_user_metadata(id, &values).unwrap();
        assert!(cx.run("workflow.metadata_save"));
    }
    harness.run_steps(3);
    assert!(harness.state().dialog_open("workflow_preset_save"));
    pin(&mut harness);
    common::snapshot::save(&mut harness, &format!("workflow-preset-save-{tag}"));
    harness.state_mut().close_dialog("workflow_preset_save");
    {
        let cx = &mut harness.state_mut().cx;
        let id = cx.doc.current_id() as i64;
        let row = cx.doc.metadata_catalog().unwrap().photo(id).unwrap().unwrap();
        let mut form = PresetSave::new(&row);
        form.name = "촬영자 정보".into();
        save_preset_done(cx, &form.name, &form.rows);
        assert!(cx.state::<WorkflowState>().errors.is_empty());
        assert_eq!(preset_names(cx), ["촬영자 정보"]);
    }

    // The list that picks a preset to apply.
    {
        let cx = &mut harness.state_mut().cx;
        assert!(cx.run("workflow.metadata_apply"));
    }
    harness.run_steps(3);
    assert!(harness.state().dialog_open("workflow_preset_choose"));
    pin(&mut harness);
    common::snapshot::save(&mut harness, &format!("workflow-preset-choose-{tag}"));
    harness.state_mut().close_dialog("workflow_preset_choose");
    {
        let cx = &mut harness.state_mut().cx;
        // Both photos are selected (the first one is open).
        let first = cx.doc.index_of(ids[0] as u64).unwrap();
        cx.select_photo(first);
        let second = cx.doc.index_of(ids[1] as u64).unwrap();
        cx.doc.toggle_selected(second);
        choose_done(cx, Choice::Apply, "촬영자 정보");
    }
    common::snapshot::settle(&mut harness);
    {
        let catalog = reader(&dir);
        for id in &ids {
            let photo = catalog.photo(*id).unwrap().unwrap();
            assert_eq!(photo["user_metadata"]["creator"], "Sample Creator", "photo {id}");
            assert_eq!(photo["user_metadata"]["copyright"], "2026 Sample", "photo {id}");
            if *id == ids[1] {
                assert!(photo["user_metadata"].get("title").is_none(), "the title was not ticked");
            }
        }
    }

    // The shortcut keyword.
    {
        let cx = &mut harness.state_mut().cx;
        assert!(cx.run("workflow.keyword_apply"), "asks for the keyword first");
    }
    harness.run_steps(3);
    assert!(harness.state().dialog_open("workflow_keyword"));
    pin(&mut harness);
    common::snapshot::save(&mut harness, &format!("workflow-keyword-{tag}"));
    harness.state_mut().close_dialog("workflow_keyword");
    {
        let cx = &mut harness.state_mut().cx;
        keyword_done(cx, Then::Apply, " 여행|일본 ");
        assert_eq!(saved_keyword(cx), "여행|일본");
    }
    common::snapshot::settle(&mut harness);
    {
        let catalog = reader(&dir);
        for id in &ids {
            assert_eq!(catalog.photo(*id).unwrap().unwrap()["keywords"], "여행|일본");
        }
    }
    assert_eq!(
        harness.state().cx.status(),
        grainy_app::text::tr_with("키워드 “{0}”를 {1}장에 추가", &["여행|일본", "2"])
    );
    LAUNCH_DISABLED.store(false, Ordering::Relaxed);
    grainy_i18n::set_language(Language::Korean);
}

fn commands_enabled(cx: &AppContext, id: &str) -> bool {
    grainy_app::commands::enabled(cx, id)
}

#[test]
fn the_window_runs_the_batches_in_korean() {
    flow(Language::Korean, "ko");
}

#[test]
fn the_window_runs_the_batches_in_english() {
    flow(Language::English, "en");
}

#[test]
fn the_boxes_have_their_ids() {
    let _turn = common::language_lock();
    let shared = Shared::new(0);
    shared.finish();
    assert!(shared.get().finished);
    let names = vec!["a".to_owned()];
    assert_eq!(grainy_app::dialogs::Dialog::id(&PresetChoose::new(Choice::Delete, names)), "workflow_preset_choose");
    assert_eq!(grainy_app::dialogs::Dialog::id(&KeywordInput::new(String::new(), Then::Set)), "workflow_keyword");
}
