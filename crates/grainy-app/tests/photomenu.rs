//! Work package 14: the photo context menu and the photo actions, against the oracle (`golden/app/photomenu`, built by
//! `tools/golden/gen_app_photomenu.py` with the Python Grainy's own `photo_actions.build_menu`, `transform_selected`,
//! `edit_selected`, `reset_selected` and `WorkflowTools.camera_orientation` run on a stand-in for the window).
//!
//! The model tests drive the context without a window; the window tests render the real application headless and
//! write `tests/snapshots/photomenu-*`. Every test works on copies and generated photos in temporary folders; the
//! folder is never opened in a file manager (the opener is a recorder).

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use eframe::egui;
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::photo_actions::{FolderOpener, PhotoActions};
use grainy_app::commands::sync::SyncState;
use grainy_app::commands::Invocation;
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::document::{Document, Query};
use grainy_app::menus_photo::{menu, Node, DEFERRED};
use grainy_app::panels::transform::UprightState;
use grainy_app::render::TestCard;
use grainy_app::session::Session;
use grainy_app::{AppContext, GrainyApp, Mode};
use grainy_catalog::rusqlite::Connection;
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

fn case() -> Value {
    Case::open("app", "photomenu").json().clone()
}

// ---- the oracle's library -----------------------------------------------------------------------------------

/// The oracle's library in a temporary folder, with the list options and the saved export settings of one state.
fn golden_library(name: &str, query: impl FnOnce(&mut Query), previous_export: bool) -> (PathBuf, AppContext) {
    let meta = case();
    let dir = common::temp_dir(name);
    std::fs::copy(grainy_testkit::root().join("golden/app/photomenu/start.sqlite"), dir.join("catalog.sqlite"))
        .expect("copy the library");
    let stamp = meta["stamp"].as_str().unwrap().to_owned();
    let token = meta["token"].as_str().unwrap().to_owned();
    let catalog = Catalog::with_sources(&dir, Sources::new(move || stamp.clone(), move || token.clone())).unwrap();
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    if previous_export {
        doc.save_preference(meta["previous_export_key"].as_str().unwrap(), &meta["previous_export"]).unwrap();
    }
    let mut options = doc.query();
    query(&mut options);
    doc.set_query(options).unwrap();
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

// ---- the menu -------------------------------------------------------------------------------------------------

/// The Rust menu against the oracle's `QMenu` dump: order, titles, data keys, separators, submenus, enabled and
/// checked states. Items of packages that are not merged yet must be present and greyed out.
fn same_menu(nodes: &[Node], expected: &Value, at: &str) {
    let expected = expected.as_array().unwrap();
    let shown: Vec<String> = nodes
        .iter()
        .map(|n| match n {
            Node::Separator => "---".to_owned(),
            Node::Item(i) => i.title.clone(),
            Node::Sub(s) => s.title.to_owned(),
        })
        .collect();
    // The golden is the oracle's menu on Windows; on macOS its item reads `'Finder에서 폴더 열기' if
    // sys.platform=='darwin' else '탐색기에서 폴더 열기'` (luma/photo_actions.py).
    let on_this_system = |title: &str| {
        if title == grainy_app::text::tr("탐색기에서 폴더 열기") {
            grainy_app::text::tr(grainy_app::system_open::SHOW_FOLDER_IN_FILE_MANAGER)
        } else {
            title.to_owned()
        }
    };
    let wanted: Vec<String> = expected
        .iter()
        .map(|e| e.as_str().map_or_else(|| on_this_system(e["title"].as_str().unwrap()), |_| "---".into()))
        .collect();
    assert_eq!(shown, wanted, "{at}: the entries in order");
    for (node, e) in nodes.iter().zip(expected) {
        match node {
            Node::Separator => assert_eq!(e, "sep", "{at}"),
            Node::Sub(sub) => {
                let name = format!("{at} / {}", sub.title);
                assert_eq!(e["sub"], true, "{name}");
                assert_eq!(sub.key, e["key"].as_str().unwrap(), "{name}: data key");
                assert_eq!(sub.enabled, e["enabled"].as_bool().unwrap(), "{name}: enabled");
                same_menu(&sub.children, &e["children"], &name);
            }
            Node::Item(item) => {
                let name = format!("{at} / {}", item.title);
                assert_eq!(e["sub"], false, "{name}");
                assert_eq!(item.key, e["key"].as_str().unwrap(), "{name}: data key");
                if DEFERRED.iter().any(|(key, _)| *key == item.key) {
                    assert!(!item.enabled && item.command.is_empty(), "{name}: waits for its package");
                } else {
                    assert_eq!(item.enabled, e["enabled"].as_bool().unwrap(), "{name}: enabled");
                }
                assert_eq!(item.checked, e["checked"].as_bool(), "{name}: checked");
                assert_eq!(item.tip, e["tip"].as_str(), "{name}: tip");
            }
        }
    }
}

/// One state of the generator in the Rust context.
fn menu_for(name: &str) -> (Vec<Node>, Value) {
    let meta = case();
    let entry = &meta["menus"][name];
    let spec = &entry["spec"];
    let plain = meta["plain_collection"].as_i64().unwrap();
    let smart = meta["smart_collection"].as_i64().unwrap();
    let collapsed = spec["collapsed"].as_bool().unwrap_or(false);
    let collection = match spec["collection"].as_str() {
        Some("plain") => Some(plain),
        Some("smart") => Some(smart),
        _ => None,
    };
    let (dir, mut cx) = golden_library(
        &format!("photomenu-{name}"),
        |q| {
            q.collapsed = collapsed;
            q.collection = collection;
        },
        spec["previous_export"].as_bool().unwrap_or(false),
    );
    open(&mut cx, spec["current"].as_u64().unwrap());
    let ids: Vec<u64> = spec["selection"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    select_ids(&mut cx, &ids);
    if spec["copied"].as_bool().unwrap_or(false) {
        assert!(cx.doc.copy());
    }
    if let Some(previous) = spec["previous"].as_u64() {
        cx.state::<SyncState>().previous = Some(previous);
    }
    if spec["exporting"].as_bool().unwrap_or(false) {
        cx.jobs.spawn(
            "export",
            "export",
            |handle| {
                while !handle.cancelled() {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                Ok(())
            },
            |_cx: &mut AppContext, _| {},
        );
    }
    let nodes = menu(&mut cx);
    cx.jobs.cancel_kind("export");
    drop(cx);
    drop(dir);
    (nodes, entry["menu"].clone())
}

#[test]
fn the_menu_of_one_photo_is_the_oracle_s() {
    let _turn = common::language_lock();
    let (nodes, expected) = menu_for("single");
    same_menu(&nodes, &expected, "single");
}

#[test]
fn the_menu_of_a_mixed_selection_is_the_oracle_s() {
    let _turn = common::language_lock();
    let (nodes, expected) = menu_for("mixed");
    same_menu(&nodes, &expected, "mixed");
}

#[test]
fn the_menu_with_stacks_collapsed_is_the_oracle_s() {
    let _turn = common::language_lock();
    let (nodes, expected) = menu_for("collapsed");
    same_menu(&nodes, &expected, "collapsed");
}

#[test]
fn the_menu_in_an_ordinary_collection_is_the_oracle_s() {
    let _turn = common::language_lock();
    let (nodes, expected) = menu_for("in_plain");
    same_menu(&nodes, &expected, "in_plain");
}

#[test]
fn the_menu_with_equal_values_checks_them() {
    let _turn = common::language_lock();
    let (nodes, expected) = menu_for("equal");
    same_menu(&nodes, &expected, "equal");
}

#[test]
fn the_menu_while_exporting_in_a_smart_collection_is_the_oracle_s() {
    let _turn = common::language_lock();
    let (nodes, expected) = menu_for("exporting");
    same_menu(&nodes, &expected, "exporting");
}

#[test]
fn the_menu_of_an_unrotated_photo_is_the_oracle_s() {
    let _turn = common::language_lock();
    let (nodes, expected) = menu_for("plain_view");
    same_menu(&nodes, &expected, "plain_view");
}

#[test]
fn the_menu_of_an_empty_library_is_all_disabled() {
    let _turn = common::language_lock();
    let meta = case();
    let dir = common::temp_dir("photomenu-none");
    let doc = CatalogDocument::open(&dir.join("Library")).unwrap();
    let mut cx = context(doc);
    let nodes = menu(&mut cx);
    same_menu_disabled(&nodes);
    // The oracle's own empty-selection menu has the same entries.
    let expected = &meta["menus"]["none"]["menu"];
    let mut cx = context(CatalogDocument::open(&dir.join("Library2")).unwrap());
    same_menu(&menu(&mut cx), expected, "none");
}

fn same_menu_disabled(nodes: &[Node]) {
    for node in nodes {
        match node {
            Node::Item(item) => assert!(!item.enabled, "{} is greyed out without photos", item.title),
            Node::Sub(sub) => {
                // Submenus other than 빠른 내보내기 stay openable in the oracle.
                same_menu_disabled(&sub.children);
            }
            Node::Separator => {}
        }
    }
}

#[test]
fn the_deferred_items_are_the_ones_of_the_packages_not_merged() {
    let _turn = common::language_lock();
    let (nodes, _) = menu_for("mixed");
    let mut found = Vec::new();
    fn walk(nodes: &[Node], found: &mut Vec<(String, bool)>) {
        for node in nodes {
            match node {
                Node::Item(item) => found.push((item.key.clone(), item.command.is_empty())),
                Node::Sub(sub) => walk(&sub.children, found),
                Node::Separator => {}
            }
        }
    }
    walk(&nodes, &mut found);
    for (key, empty) in found {
        assert_eq!(empty, DEFERRED.iter().any(|(k, _)| *k == key), "{key}");
    }
    assert_eq!(DEFERRED.len(), 0);
}

// ---- the actions ----------------------------------------------------------------------------------------------

fn frame(cx: &mut AppContext) {
    cx.doc.flush().unwrap();
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
        "rotate_right" => assert!(cx.run("photo.rotate_right")),
        "rotate_left" => assert!(cx.run("photo.rotate_left")),
        "flip" => assert!(cx.run("photo.flip")),
        "flip_vertical" => assert!(cx.run("photo.flip_vertical")),
        "camera" => assert!(cx.run("photo.camera_orientation")),
        "mono" => assert!(cx.run_with("photo.monochrome", &Invocation::value(a[1].as_bool().unwrap()))),
        "reset" => assert!(cx.run("photo.reset")),
        "undo" => assert!(cx.run("develop.undo")),
        other => panic!("unknown step {other}"),
    }
    frame(cx);
}

#[test]
fn the_scripted_session_writes_what_the_oracle_writes() {
    // The status lines are compared in Korean: not while a window test renders in English.
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let meta = case();
    let session = &meta["session"];
    let (dir, mut cx) = golden_library("photomenu-session", |_| {}, false);
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
        let at = format!("{step}");
        let last = expected["messages"].as_array().and_then(|m| m.last()).and_then(Value::as_str);
        assert_eq!(cx.status(), last.unwrap_or(""), "{at}: the status line");
        assert!(expected["error"].is_null(), "{at}");
        assert_eq!(
            cx.state::<UprightState>().generation,
            expected["upright_generation"].as_u64().unwrap(),
            "{at}: alignment results made stale"
        );
    }
    cx.doc.edit_mut().commit("보정 변경");
    frame(&mut cx);
    equal(&project(&path), &session["final"], "final");
}

#[test]
fn a_turn_clears_the_crop_and_the_view_fits() {
    let _turn = common::language_lock();
    let (_dir, mut cx) = golden_library("photomenu-fit", |_| {}, false);
    open(&mut cx, 1);
    cx.set_zoom(grainy_app::Zoom::Actual);
    assert!(cx.run("photo.rotate_right"));
    frame(&mut cx);
    assert_eq!(cx.zoom(), grainy_app::Zoom::Fit);
    assert_eq!(cx.doc.edit().settings().i64("rotation"), 2);
    assert!(cx.doc.edit().settings().is_none("crop"), "the crop belongs to the rotated frame");
    assert!(!cx.before());
}

// ---- a folder opener that never starts the file manager -------------------------------------------------

#[derive(Clone, Default)]
struct Recorder(Arc<Mutex<Vec<PathBuf>>>);

impl FolderOpener for Recorder {
    fn open(&mut self, folder: &Path) {
        self.0.lock().unwrap().push(folder.to_path_buf());
    }
}

fn session_context(name: &str, count: usize) -> (AppContext, Vec<PathBuf>) {
    let dir = common::temp_dir(name);
    let paths = common::eight_photos(&dir)[..count].to_vec();
    let mut session = Session::default();
    session.add(paths.clone());
    (context(session), paths)
}

#[test]
fn the_folder_opens_through_the_opener() {
    let _turn = common::language_lock();
    let (mut cx, paths) = session_context("photomenu-folder", 3);
    let recorder = Recorder::default();
    cx.state::<PhotoActions>().opener = Some(Box::new(recorder.clone()));
    assert!(cx.run("photo.show_folder"));
    cx.select_photo(2);
    assert!(cx.run("photo.show_folder"));
    let opened = recorder.0.lock().unwrap().clone();
    assert_eq!(opened, vec![paths[0].parent().unwrap().to_path_buf(); 2], "the open photo's folder, nothing started");
}

#[test]
fn the_develop_item_opens_the_photo_large() {
    let _turn = common::language_lock();
    let (mut cx, _) = session_context("photomenu-develop", 3);
    cx.set_mode(Mode::Library);
    cx.set_zoom(grainy_app::Zoom::Actual);
    assert!(cx.run("photo.develop"));
    assert_eq!(cx.mode(), Mode::Develop);
    assert_eq!(cx.zoom(), grainy_app::Zoom::Fit);
}

#[test]
fn a_session_turns_flips_converts_restores_and_resets() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let (mut cx, _) = session_context("photomenu-session-doc", 4);
    let setting = |cx: &AppContext, index: usize, key: &str| cx.doc.settings_of(index).i64(key);
    cx.doc.edit_mut().set("exposure", 0.4);
    cx.doc.edit_mut().commit("보정 변경");
    for index in [1, 2] {
        cx.doc.toggle_selected(index);
    }
    assert!(cx.run("photo.rotate_right"));
    frame(&mut cx);
    assert_eq!([0, 1, 2].map(|i| setting(&cx, i, "rotation")), [1, 1, 1]);
    assert_eq!(setting(&cx, 3, "rotation"), 0, "not selected");
    assert_eq!(cx.status(), "3장의 사진에 적용했습니다. 사진별 실행 취소로 되돌릴 수 있습니다.");
    assert!(cx.run("photo.flip_vertical"));
    frame(&mut cx);
    assert_eq!([0, 1, 2].map(|i| setting(&cx, i, "rotation")), [3, 3, 3]);
    assert!([0, 1, 2].iter().all(|i| cx.doc.settings_of(*i).bool("flip")));
    assert!(cx.run("photo.camera_orientation"));
    frame(&mut cx);
    assert_eq!([0, 1, 2].map(|i| setting(&cx, i, "rotation")), [0, 0, 0]);
    assert!([0, 1, 2].iter().all(|i| !cx.doc.settings_of(*i).bool("flip")));
    assert_eq!(cx.status(), "카메라 방향 복원 · 3장 변경");
    assert!(cx.run_with("photo.monochrome", &Invocation::value(true)));
    frame(&mut cx);
    assert!([0, 1, 2].iter().all(|i| cx.doc.settings_of(*i).bool("monochrome")));
    assert!(cx.run_with("photo.monochrome", &Invocation::value(false)));
    frame(&mut cx);
    assert!([0, 1, 2].iter().all(|i| !cx.doc.settings_of(*i).bool("monochrome")));
    assert!(cx.run("photo.reset"));
    frame(&mut cx);
    assert_eq!(cx.doc.settings_of(0).f64("exposure"), 0.0);
    assert_eq!(cx.state::<UprightState>().generation, 1);
}

#[test]
fn a_session_menu_greys_out_what_needs_a_library() {
    let _turn = common::language_lock();
    let (mut cx, _) = session_context("photomenu-session-menu", 3);
    cx.doc.toggle_selected(1);
    let nodes = menu(&mut cx);
    assert!(!cx.dialog_open("collections_error"), "a session has no catalogue to ask");
    fn find<'a>(nodes: &'a [Node], key: &str) -> &'a grainy_app::menus_photo::Item {
        for node in nodes {
            match node {
                Node::Item(item) if item.key == key => return item,
                Node::Sub(sub) => {
                    if let Some(found) = sub.children.iter().find_map(|n| match n {
                        Node::Item(i) if i.key == key => Some(i),
                        _ => None,
                    }) {
                        return found;
                    }
                }
                _ => {}
            }
        }
        panic!("{key} is in the menu");
    }
    assert!(find(&nodes, "rotate-right").enabled);
    assert!(find(&nodes, "sync").enabled, "two photos are selected");
    assert!(!find(&nodes, "stack").enabled, "stacks need a library");
    assert!(!find(&nodes, "collection-add").enabled);
    assert!(!find(&nodes, "restore-snapshot").enabled);
    assert!(find(&nodes, "show-folder").enabled);
}

#[test]
fn a_read_only_library_turns_photos_in_memory_only() {
    let _turn = common::language_lock();
    let dir = common::temp_dir("photomenu-read-only");
    let library = dir.join("Library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::copy(grainy_testkit::root().join("golden/app/photomenu/start.sqlite"), library.join("catalog.sqlite"))
        .unwrap();
    let original = std::fs::read(library.join("catalog.sqlite")).unwrap();
    let mut doc = CatalogDocument::open_read_only(&library, &dir.join("scratch")).unwrap();
    doc.refresh_now().unwrap();
    let mut cx = context(doc);
    open(&mut cx, 1);
    select_ids(&mut cx, &[1, 2, 3]);
    assert!(cx.run("photo.rotate_right"));
    frame(&mut cx);
    assert!(cx.run("photo.camera_orientation"));
    frame(&mut cx);
    assert_eq!(cx.doc.settings_of(0).i64("rotation"), 0, "restored in memory");
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

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) {
    common::snapshot::save(harness, name);
}

fn right_click(harness: &mut Harness<'_, GrainyApp>, pos: egui::Pos2) {
    harness.hover_at(pos);
    harness.run_steps(1);
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(1);
    }
}

/// The library of eight photos with three selected (one rotated), the menu of the photo at `cell` open.
fn menu_snapshot(language: Language, name: &str, scene: &str, mode: Mode, at: egui::Pos2, submenu: Option<egui::Pos2>) {
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
        app.set_mode(mode);
        app
    });
    common::snapshot::settle_frame(&mut harness);
    assert_eq!(harness.state().document().len(), 8, "{}", harness.state().status());
    {
        let app = harness.state_mut();
        app.edit_state_mut().set("rotation", 1);
        app.edit_state_mut().commit("보정 변경");
        for index in [1, 2] {
            app.document_mut().toggle_selected(index);
        }
        assert!(app.run("develop.copy_edits"));
    }
    common::snapshot::settle_frame(&mut harness);
    common::snapshot::open_folders(harness.state_mut(), &[]);
    right_click(&mut harness, at);
    // The right-click opened another photo: wait for its picture, then pin the status line.
    common::snapshot::settle_frame(&mut harness);
    common::snapshot::pin_status(harness.state_mut(), "");
    harness.run_steps(3);
    assert!(
        harness.query_by_label_contains(&grainy_i18n::tr("현상에서 크게 보기")).is_some(),
        "the context menu is open"
    );
    if let Some(item) = submenu {
        harness.hover_at(item);
        harness.run_steps(4);
    }
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

const AT: egui::Pos2 = egui::pos2(620.0, 220.0);
const EDITS: egui::Pos2 = egui::pos2(660.0, 555.0);

#[test]
fn the_grid_menu_korean() {
    menu_snapshot(Language::Korean, "photomenu-grid-ko", "photomenu-grid-ko", Mode::Library, AT, None);
}

#[test]
fn the_grid_menu_english() {
    menu_snapshot(Language::English, "photomenu-grid-en", "photomenu-grid-en", Mode::Library, AT, None);
}

#[test]
fn the_edit_settings_submenu_korean() {
    menu_snapshot(Language::Korean, "photomenu-edits-ko", "photomenu-edits-ko", Mode::Library, AT, Some(EDITS));
}

#[test]
fn the_edit_settings_submenu_english() {
    menu_snapshot(Language::English, "photomenu-edits-en", "photomenu-edits-en", Mode::Library, AT, Some(EDITS));
}
