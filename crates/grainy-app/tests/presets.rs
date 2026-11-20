//! Presets against the oracle (`golden/app/presets`, built by `tools/golden/gen_app.py` with the Python Grainy's
//! own `MainWindow.apply_preset` / `save_preset` and `LibraryManager.import_preset` / `export_preset` /
//! `delete_preset`): after the same scripted steps (apply built-in presets, save the current edit, apply it to a
//! photo with other geometry, import `.json`, `.xmp` (Grainy and Adobe) and `.lrtemplate` files with the review
//! box accepted and cancelled, files that are not presets, export, delete, undo and redo) the catalogue tables
//! of the Rust document are the oracle's. Also: the memory store of a session behaves like the catalogue, and the
//! window's tree, boxes and jobs run end to end on a library in a temporary folder (snapshots in Korean and
//! English).
//!
//! Every test works on copies in temporary folders; the Python Grainy's own folders are never opened.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::presets::{self, Outcome, PresetState, Store};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::document::Document;
use grainy_app::GrainyApp;
use grainy_catalog::rusqlite::{types::ValueRef, Connection};
use grainy_catalog::{Catalog, Sources};
use grainy_core::{normalized, Settings};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

fn case() -> Value {
    Case::open("app", "presets").json().clone()
}

fn now() -> chrono::NaiveDateTime {
    chrono::NaiveDateTime::parse_from_str("2026-11-13 01:02:03", "%Y-%m-%d %H:%M:%S").unwrap()
}

/// A copy of the oracle's library (the one of `tests/library.rs`) in a fresh folder, opened with the oracle's
/// clock and token.
fn golden_library(name: &str) -> (PathBuf, CatalogDocument) {
    let meta = case();
    let dir = common::temp_dir(name);
    std::fs::copy(grainy_testkit::root().join("golden/app/library/start.sqlite"), dir.join("catalog.sqlite"))
        .expect("copy the library");
    let stamp = meta["stamp"].as_str().unwrap().to_owned();
    let token = meta["token"].as_str().unwrap().to_owned();
    let catalog = Catalog::with_sources(&dir, Sources::new(move || stamp.clone(), move || token.clone())).unwrap();
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.set_clock(Arc::new(now));
    doc.refresh_now().unwrap();
    (dir, doc)
}

fn files_folder() -> PathBuf {
    grainy_testkit::root().join("golden/app/presets/files")
}

fn open(doc: &mut CatalogDocument, id: u64) {
    if doc.index_of(id).is_none() {
        doc.set_query(grainy_app::document::Query::default()).unwrap();
        doc.refresh_now().unwrap();
    }
    let index = doc.index_of(id).unwrap_or_else(|| panic!("photo {id} listed"));
    doc.select(index);
    assert_eq!(doc.current_id(), id);
}

/// The selected tables of a catalogue, rows ordered by all columns (as the generator dumps them), without the
/// photos' `metadata` (it carries the generating run's file times).
fn dump(path: &Path, token: &str, keep: &[&str]) -> Value {
    let db = Connection::open(path).unwrap();
    let mut out = Map::new();
    for table in keep {
        let columns: Vec<String> = db
            .prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |r| r.get(1))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let order = columns.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(",");
        let mut statement = db.prepare(&format!("SELECT * FROM {table} ORDER BY {order}")).unwrap();
        let rows: Vec<Value> = statement
            .query_map([], |r| {
                let mut row = Map::new();
                for (i, column) in columns.iter().enumerate() {
                    if *table == "photos" && column == "metadata" {
                        continue;
                    }
                    let value = if *table == "preview_state" && column == "token" {
                        json!(token)
                    } else {
                        match r.get_ref(i)? {
                            ValueRef::Null => Value::Null,
                            ValueRef::Integer(v) => json!(v),
                            ValueRef::Real(v) => json!(v),
                            ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                            ValueRef::Blob(b) => {
                                json!({"blob": b.iter().map(|b| format!("{b:02x}")).collect::<String>()})
                            }
                        }
                    };
                    row.insert(column.clone(), value);
                }
                Ok(Value::Object(row))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        out.insert((*table).to_owned(), Value::Array(rows));
    }
    Value::Object(out)
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
            assert_eq!(a.len(), b.len(), "{at}: length");
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                equal(x, y, &format!("{at}[{i}]"));
            }
        }
        _ => panic!("{at}\n  rust:   {actual}\n  oracle: {expected}"),
    }
}

fn names(store: &Store) -> Vec<String> {
    store.list().unwrap().into_iter().map(|(n, _)| n).collect()
}

/// What a script step did, for comparing with the oracle's log entry.
#[derive(Default)]
struct Done {
    error: Option<String>,
    review: Option<presets::Review>,
    exported: Option<Value>,
}

/// One step of the generator's preset script through the document and the preset functions, as the window's
/// commands do them.
fn step(doc: &mut CatalogDocument, dir: &Path, exported_to: &Path, step: &Value) -> Done {
    let a = step.as_array().unwrap();
    let mut done = Done::default();
    let mut store = Store::open(Some(dir), &[]).unwrap();
    match a[0].as_str().unwrap() {
        "open" => open(doc, a[1].as_u64().unwrap()),
        "set" => doc.edit_mut().set(a[1].as_str().unwrap(), a[2].clone()),
        "commit" => {
            doc.edit_mut().commit(a.get(1).and_then(Value::as_str).unwrap_or("보정 변경"));
        }
        "undo" => {
            doc.edit_mut().undo();
        }
        "redo" => {
            doc.edit_mut().redo();
        }
        "apply" => {
            let name = a[1].as_str().unwrap();
            let all = presets::all_presets(&store.list().unwrap());
            let (_, preset) = all.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("preset {name}"));
            let info = presets::photo_info(Some(dir), doc.current_id(), &doc.current().unwrap().path);
            if let Err(message) = presets::apply_to(doc, preset, &info) {
                done.error = Some(message);
            }
        }
        "save" => {
            let name = a[1].as_str().unwrap().trim();
            if !name.is_empty() {
                store.save(name, &doc.edit().settings().clone()).unwrap();
            }
        }
        "import" => {
            let accept = a.get(2).and_then(Value::as_bool).unwrap_or(true);
            match presets::import_file(&mut store, &files_folder().join(a[1].as_str().unwrap())) {
                Ok(Outcome::Saved(_)) => {}
                Ok(Outcome::Review(review)) => {
                    if accept {
                        presets::add_reviewed(&mut store, &review).unwrap();
                    }
                    done.review = Some(review);
                }
                Err(message) => done.error = Some(message),
            }
        }
        "export" => {
            let path = exported_to.join(a[1].as_str().unwrap());
            std::fs::write(&path, presets::export_text(&path, doc.edit().settings())).unwrap();
            done.exported = Some(serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap());
        }
        "delete" => store.delete(a[1].as_str().unwrap()).unwrap(),
        other => panic!("unknown step {other}"),
    }
    Document::flush(doc).unwrap();
    done
}

#[test]
fn a_scripted_preset_session_writes_what_the_oracle_writes() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let meta = case();
    let (dir, mut doc) = golden_library("presets-session");
    let exported = common::temp_dir("presets-exported");
    let log = meta["log"].as_array().unwrap();
    assert_eq!(log.len(), meta["script"].as_array().unwrap().len());
    let (mut reviews, mut errors, mut saved) = (0, 0, 0);
    for (i, (s, expected)) in meta["script"].as_array().unwrap().iter().zip(log).enumerate() {
        let done = step(&mut doc, &dir, &exported, s);
        println!("step {i}: {s}");
        // Errors: the oracle's message where it has none of the machine's paths.
        let expected_error = expected.get("error").or_else(|| expected.get("shown_errors"));
        match (&done.error, expected_error) {
            (None, None) => {}
            (Some(message), Some(error)) => {
                errors += 1;
                let kind = error["type"].as_str().unwrap_or("");
                let oracle = error["message"].as_str().unwrap();
                if kind != "FileNotFoundError" && kind != "OSError" {
                    assert_eq!(message, oracle, "step {i}: {s}");
                } else {
                    // The oracle's temporary folder is stored as `<tmp>`; the operating system's wording
                    // differs from Python's, so only the masked shape and the file name are compared.
                    let own = message.replace(&files_folder().display().to_string(), "<tmp>");
                    assert!(oracle.starts_with("[Errno 2] No such file or directory: '<tmp>"), "{oracle}");
                    assert!(oracle.ends_with("missing.json'"), "{oracle}");
                    assert!(!own.is_empty() && !own.contains(&files_folder().display().to_string()), "step {i}: {own}");
                }
            }
            (rust, oracle) => panic!("step {i}: {s}\n  rust error:   {rust:?}\n  oracle error: {oracle:?}"),
        }
        // The review box: what it says and what the details say.
        match (&done.review, expected.get("review")) {
            (None, None) => {}
            (Some(review), Some(box_)) => {
                reviews += 1;
                let text = grainy_i18n::tr("“{0}” 프리셋을 추가합니다.\n");
                let text = grainy_i18n::format(&text, &[&review.name]).unwrap() + &review.preview;
                assert_eq!(text, box_["text"].as_str().unwrap(), "step {i}: the box's text");
                match box_.get("details") {
                    Some(details) => {
                        assert_ne!(review.preview, review.report);
                        assert_eq!(review.report, details.as_str().unwrap(), "step {i}: the details");
                    }
                    None => assert_eq!(review.preview, review.report, "step {i}: no details when the same"),
                }
                assert_eq!(box_["title"], "Adobe 프리셋 가져오기");
                assert_eq!(box_["labels"]["1"], "프리셋 추가");
                assert_eq!(box_["labels"]["2"], "취소");
            }
            (rust, oracle) => panic!("step {i}: {s}\n  rust review:   {rust:?}\n  oracle review: {oracle:?}"),
        }
        if let Some(file) = expected.get("exported") {
            saved += 1;
            equal(done.exported.as_ref().unwrap(), file, "the exported file");
        }
        let store = Store::open(Some(&dir), &[]).unwrap();
        let expected_names: Vec<&str> =
            expected["presets"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert_eq!(names(&store), expected_names, "step {i}: {s}: the presets after it");
    }
    doc.edit_mut().commit("보정 변경");
    Document::flush(&mut doc).unwrap();
    doc.refresh_now().unwrap();
    let keep: Vec<&str> = meta["tables"].as_object().unwrap().keys().map(String::as_str).collect();
    let tables = dump(&dir.join("catalog.sqlite"), meta["token"].as_str().unwrap(), &keep);
    for (table, expected) in meta["tables"].as_object().unwrap() {
        equal(&tables[table], expected, table);
        println!("{table}: {} rows equal to the oracle's", expected.as_array().map_or(0, Vec::len));
    }
    println!("{reviews} review boxes, {errors} refused, {saved} export equal to the oracle's");
    assert!(reviews >= 4 && errors >= 5 && saved == 1);
}

#[test]
fn the_built_in_presets_are_the_oracles() {
    let meta = case();
    let expected = meta["built_in"].as_array().unwrap();
    let ours = presets::built_in();
    assert_eq!(ours.len(), expected.len());
    for (mine, theirs) in ours.iter().zip(expected) {
        assert_eq!(mine.0, theirs[0].as_str().unwrap());
        equal(&mine.1, &theirs[1], &mine.0);
    }
}

#[test]
fn a_user_preset_named_like_a_built_in_one_takes_its_place() {
    let user = vec![("Amber film".to_owned(), json!({"exposure": 1.0})), ("Zed".to_owned(), json!({}))];
    let all = presets::all_presets(&user);
    let names: Vec<&str> = all.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        ["원본의 색", "Soft daylight", "Amber film", "Silver / B&W", "Crisp landscape", "Quiet blue", "Zed"]
    );
    assert_eq!(all[2].1, json!({"exposure": 1.0}));
}

/// The memory store of a session and the catalogue do the same: save (geometry reset), Adobe presets, delete.
#[test]
fn the_memory_store_does_what_the_catalogue_does() {
    let _turn = common::language_lock();
    let dir = common::temp_dir("presets-memory");
    let mut library = Store::open(Some(&dir), &[]).unwrap();
    let mut memory = Store::open(None, &[]).unwrap();
    let mut edited = normalized(&Map::new());
    edited.set("exposure", 0.5);
    edited.set("crop", json!([0.1, 0.1, 0.9, 0.9]));
    edited.set("rotation", 2);
    edited.set("flip", true);
    let adobe = grainy_adobe::preset::read(&files_folder().join("adobe look.xmp")).unwrap();
    for store in [&mut library, &mut memory] {
        store.save("b", &edited).unwrap();
        store.save("a", &edited).unwrap();
        store.save_adobe("c", &adobe).unwrap();
        store.save("b", &normalized(&Map::new())).unwrap();
        store.delete("a").unwrap();
    }
    let (l, m) = (library.list().unwrap(), memory.list().unwrap());
    assert_eq!(l, m);
    assert_eq!(l.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["b", "c"]);
    assert!(l[1].1.get(presets::KEY).is_some());
    // A preset keeps nothing of the photo's geometry.
    library.save("geometry", &edited).unwrap();
    let saved = library.list().unwrap().into_iter().find(|(n, _)| n == "geometry").unwrap().1;
    assert_eq!(saved["crop"], Value::Null);
    assert_eq!(saved["rotation"], 0);
    assert_eq!(saved["flip"], false);
    assert_eq!(saved["exposure"], 0.5);
}

#[test]
fn files_that_are_not_presets_are_refused_and_nothing_is_saved() {
    let mut store = Store::open(None, &[]).unwrap();
    let folder = files_folder();
    for (file, message) in [
        ("no settings.json", "Luma 보정값이 들어 있는 프리셋이 아닙니다"),
        ("empty settings.json", "Luma 보정값이 들어 있는 프리셋이 아닙니다"),
        ("array.json", "'list' object has no attribute 'get'"),
    ] {
        let error = presets::import_file(&mut store, &folder.join(file)).unwrap_err();
        assert!(error.starts_with(message), "{file}: {error}");
    }
    assert!(presets::import_file(&mut store, &folder.join("junk.xmp")).is_err());
    assert!(presets::import_file(&mut store, &folder.join("missing.json")).is_err());
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn an_exported_preset_imports_again_under_its_file_name() {
    let dir = common::temp_dir("presets-roundtrip");
    let mut settings = normalized(&Map::new());
    settings.set("exposure", 0.35);
    settings.set("film_look", "kodak-portra-400");
    settings.set("grain", 14);
    let path = dir.join("preset.luma.json");
    std::fs::write(&path, presets::export_text(&path, &settings)).unwrap();
    let mut store = Store::open(None, &[]).unwrap();
    let Outcome::Saved(name) = presets::import_file(&mut store, &path).unwrap() else { panic!("a Grainy preset") };
    // The file's name is `preset.luma`; the export names the preset after it.
    assert_eq!(name, "preset.luma");
    let stored = store.list().unwrap().remove(0).1;
    for key in ["exposure", "film_look", "grain"] {
        assert_eq!(stored[key], settings.value(key).clone(), "{key}");
    }
}

#[test]
fn applying_keeps_the_photos_geometry_and_fits_the_preset() {
    let mut doc = grainy_app::context::memory_document();
    let folder = common::temp_dir("presets-apply");
    let path = folder.join("garden.png");
    common::save_png(&common::garden(1, [1.0; 3], false), &path);
    doc.add_files(vec![path.clone()], &[]).unwrap();
    doc.select(0);
    doc.edit_mut().set("crop", json!([0.1, 0.2, 0.8, 0.9]));
    doc.edit_mut().set("rotation", 3);
    doc.edit_mut().set("straighten", 2.5);
    doc.edit_mut().set("flip", true);
    doc.edit_mut().set("exposure", 1.5);
    doc.edit_mut().commit("보정 변경");
    let preset = presets::built_in().iter().find(|(n, _)| n == "Amber film").unwrap().1.clone();
    presets::apply_to(doc.as_mut(), &preset, &json!({"format": "png"})).unwrap();
    let after = doc.edit().settings().clone();
    assert_eq!(after.value("crop"), &json!([0.1, 0.2, 0.8, 0.9]));
    assert_eq!(after.i64("rotation"), 3);
    assert_eq!(after.f64("straighten"), 2.5);
    assert!(after.bool("flip"));
    // The preset replaces the rest: its values, and the defaults for everything it does not name.
    assert_eq!(after.f64("temperature"), 16.0);
    assert_eq!(after.f64("exposure"), 0.0);
    assert_eq!(doc.edit().history()[0], "보정 변경");
    assert_eq!(doc.edit().depth().0, 2);
    // A lens profile that belongs to another lens is not carried over.
    let mut lens = normalized(&Map::new());
    lens.set("lensfun", json!({"maker": "Canon", "name": "Canon EF 50mm f/1.8", "xml": "<lens/>"}));
    let error = presets::applied(&Value::Object(lens.0.clone()), &after, &path, &json!({"lens": "Sony FE 35mm F1.8"}))
        .unwrap_err();
    assert!(error.contains("렌즈"), "{error}");
    assert!(presets::applied(&Value::Object(lens.0), &after, &path, &json!({"lens": "Canon EF 50mm f/1.8"})).is_ok());
}

// ---- the window: the tree, the boxes and the jobs ---------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The window on a new library in a temporary folder with one generated photo open.
fn library_harness<'a>(name: &str) -> (Harness<'a, GrainyApp>, PathBuf) {
    let dir = common::temp_dir(name);
    let photo = dir.join("garden.png");
    common::save_png(&common::garden(2, [1.0; 3], false), &photo);
    let library = dir.join("Library");
    let data = common::data_dir(&dir);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_library(&cc.egui_ctx, &library, Some(data));
        app.set_debug_overlay(false);
        app.open_paths(vec![photo]);
        app
    });
    settle(&mut harness);
    // The folders that opened by themselves while the import's listing grew are not the same in every run.
    common::snapshot::open_folders(harness.state_mut(), &[name]);
    settle(&mut harness);
    (harness, dir)
}

fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle(harness);
}

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) {
    common::snapshot::save(harness, name);
}

fn user_names(harness: &Harness<'_, GrainyApp>) -> Vec<String> {
    harness
        .state()
        .state_ref::<PresetState>()
        .map(|s| s.user.iter().map(|(n, _)| n.clone()).collect())
        .unwrap_or_default()
}

fn settings_of(harness: &Harness<'_, GrainyApp>) -> Settings {
    harness.state().settings().clone()
}

/// The whole path through the window: import an Adobe preset (review box), add it, apply it, save the edit
/// under a name, delete it; the tree after each.
fn window_flow(language: Language, tag: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let (mut harness, dir) = library_harness(&format!("presets-window-{tag}"));
    harness.state_mut().open_foldout("presets");
    settle(&mut harness);
    assert!(user_names(&harness).is_empty());

    // The tree: a maker and the other presets open.
    {
        let state = harness.state_mut().state::<PresetState>();
        state.open.insert("Kodak".to_owned());
        state.open.insert("others".to_owned());
    }
    harness.run_steps(3);
    save(&mut harness, &format!("presets-tree-{tag}"));

    // Importing an Adobe preset opens the review box.
    let adobe = files_folder().join("adobe look.xmp");
    presets::import_path(&mut harness.state_mut().cx, adobe.clone());
    settle(&mut harness);
    assert!(harness.state().dialog_open("preset_review"), "the review box opened");
    assert!(user_names(&harness).is_empty(), "nothing is added before the box is accepted");
    save(&mut harness, &format!("presets-review-{tag}"));
    harness.state_mut().close_dialog("preset_review");

    // 프리셋 추가: the preset joins the list with its report as tooltip.
    let mut store = Store::open(Some(&dir.join("Library")), &[]).unwrap();
    let Ok(Outcome::Review(review)) = presets::import_file(&mut store, &adobe) else { panic!("an Adobe preset") };
    presets::add_review(&mut harness.state_mut().cx, review);
    settle(&mut harness);
    assert_eq!(user_names(&harness), ["adobe look"]);

    // Applying it: its values, one undo step, the photo's own geometry.
    harness.state_mut().edit_state_mut().set("crop", json!([0.1, 0.1, 0.9, 0.9]));
    harness.state_mut().edit_state_mut().commit("보정 변경");
    presets::apply(&mut harness.state_mut().cx, "adobe look");
    settle(&mut harness);
    let applied = settings_of(&harness);
    assert!((applied.f64("exposure") - 0.0).abs() > 1e-6 || applied.f64("contrast") != 0.0, "the preset's values");
    assert_eq!(applied.value("crop"), &json!([0.1, 0.1, 0.9, 0.9]));
    assert_eq!(harness.state().edit_state().history()[0], "보정 변경");

    // Saving the edit under a name (stripped) puts it in the other-presets folder.
    presets::save_named(&mut harness.state_mut().cx, "  My edit  ");
    settle(&mut harness);
    assert_eq!(user_names(&harness), ["My edit", "adobe look"]);
    save(&mut harness, &format!("presets-saved-{tag}"));

    // Film looks are set from the tree.
    presets::film(&mut harness.state_mut().cx, "kodak-portra-400");
    settle(&mut harness);
    assert_eq!(settings_of(&harness).str("film_look"), "kodak-portra-400");

    // Deleting it.
    presets::delete_named(&mut harness.state_mut().cx, "adobe look".to_owned());
    settle(&mut harness);
    assert_eq!(user_names(&harness), ["My edit"]);

    // The catalogue holds what the tree shows.
    let store = Store::open(Some(&dir.join("Library")), &[]).unwrap();
    assert_eq!(names(&store), ["My edit"]);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_window_runs_the_preset_commands_korean() {
    window_flow(Language::Korean, "ko");
}

#[test]
fn the_window_runs_the_preset_commands_english() {
    window_flow(Language::English, "en");
}

/// A session (no catalogue) keeps its presets in memory; the save, delete and error boxes open.
#[test]
fn a_session_keeps_presets_in_memory_and_the_boxes_open() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("presets-session-window");
    let photo = dir.join("garden.png");
    common::save_png(&common::garden(2, [1.0; 3], false), &photo);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.open_paths(vec![photo]);
        app
    });
    settle(&mut harness);
    harness.state_mut().open_foldout("presets");
    harness.run_steps(2);
    // Deleting with no user presets says so.
    assert!(harness.state_mut().run("presets.delete"));
    settle(&mut harness);
    assert!(harness.state().dialog_open("preset_error"));
    save(&mut harness, "presets-error-ko");
    harness.state_mut().close_dialog("preset_error");
    // The name box opens for a photo; its answer saves in memory.
    assert!(harness.state_mut().run("presets.save"));
    harness.run_steps(3);
    assert!(harness.state().dialog_open("preset_name"));
    save(&mut harness, "presets-name-ko");
    harness.state_mut().close_dialog("preset_name");
    presets::save_named(&mut harness.state_mut().cx, "Session look");
    settle(&mut harness);
    assert_eq!(user_names(&harness), ["Session look"]);
    // The choice box lists it.
    assert!(harness.state_mut().run("presets.delete"));
    settle(&mut harness);
    assert!(harness.state().dialog_open("preset_delete"));
    save(&mut harness, "presets-delete-ko");
    harness.state_mut().close_dialog("preset_delete");
    presets::delete_named(&mut harness.state_mut().cx, "Session look".to_owned());
    settle(&mut harness);
    assert!(user_names(&harness).is_empty());
}

/// Export and import files go through the window's jobs and the file dialog's answer.
#[test]
fn the_file_dialogs_answers_run_import_and_export() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let (mut harness, dir) = library_harness("presets-files");
    let target = dir.join("out.luma.json");
    // 프리셋 파일 내보내기 → the save dialog answered.
    let (wait, answer) = grainy_app::dialogs::preset_review::FileWait::answered(presets::export_to);
    harness.state_mut().open_dialog(Box::new(wait));
    answer.send(Some(target.clone())).unwrap();
    settle(&mut harness);
    let written: Value = serde_json::from_slice(&std::fs::read(&target).expect("the file was written")).unwrap();
    assert_eq!(written["name"], "out.luma");
    assert!(written["settings"].is_object());
    // 프리셋 파일 가져오기 → the open dialog answered with that file.
    let (wait, answer) = grainy_app::dialogs::preset_review::FileWait::answered(presets::import_path);
    harness.state_mut().open_dialog(Box::new(wait));
    answer.send(Some(target)).unwrap();
    settle(&mut harness);
    assert_eq!(user_names(&harness), ["out.luma"]);
    // A cancelled dialog does nothing.
    let (wait, answer) = grainy_app::dialogs::preset_review::FileWait::answered(presets::import_path);
    harness.state_mut().open_dialog(Box::new(wait));
    answer.send(None).unwrap();
    settle(&mut harness);
    assert_eq!(user_names(&harness), ["out.luma"]);
    assert!(!harness.state().dialog_open("preset_file"));
}
