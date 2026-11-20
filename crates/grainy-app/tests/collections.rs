//! Work package 15, collections and filters, against the oracle (`golden/app/collections_*`, built by
//! `tools/golden/gen_app_collections.py` with the Python Grainy's own `FilterDialog`, `LibraryManager` and
//! `WorkflowTools` methods and its `library_query.query`):
//!
//! * the 상세 필터 dialog: what it shows for given rules and the rules or error text it answers;
//! * the listing for every filter field and for the complex rules of smart collections;
//! * scripted sessions (create, nest, add, remove, edit, quick collection, containing collection, clear) after
//!   which the manager state, the tree, the listing and the catalogue tables are the oracle's;
//! * the window: the fold-out, the boxes, the commands (snapshots in Korean and English).
//!
//! Every test works on copies and generated photos in temporary folders.

mod common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::collections::{self as coll, CollectionsState};
use grainy_app::dialogs::filter::{FilterForm, FIELDS};
use grainy_app::document::catalog::collections::{CollectionOp, CollectionReply};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::document::{Collection, Document, Query};
use grainy_app::panels::collections::rows;
use grainy_app::render::TestCard;
use grainy_app::{AppContext, GrainyApp};
use grainy_catalog::json::dumps;
use grainy_catalog::rusqlite::{types::ValueRef, Connection};
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

fn now() -> chrono::NaiveDateTime {
    chrono::NaiveDateTime::parse_from_str("2026-11-13 01:02:03", "%Y-%m-%d %H:%M:%S").unwrap()
}

/// A copy of the oracle's library in a fresh folder, opened with the oracle's clock and token.
fn golden_library(name: &str) -> (PathBuf, CatalogDocument) {
    let meta = Case::open("app", "library").json().clone();
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

fn context(doc: CatalogDocument) -> AppContext {
    AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc))
}

/// A JSON object text of the generator, with its key order.
fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

// ---- the dialog ---------------------------------------------------------------------------------------------

fn field_index(key: &str) -> Option<usize> {
    FIELDS.iter().position(|(k, _)| *k == key)
}

/// `FilterDialog` built on rules and edited as the generator edits it: the first state and the answer.
#[test]
fn the_filter_dialog_shows_and_answers_what_the_oracle_does() {
    let golden = Case::open("app", "collections_dialog").json().clone();
    let keys: Vec<&str> = golden["fields"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect();
    assert_eq!(keys, FIELDS.iter().map(|(k, _)| *k).collect::<Vec<_>>(), "the fields and their order");
    let cases = golden["cases"].as_array().unwrap();
    assert!(cases.len() > 30);
    let mut errors = 0;
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let initial = parse(case["initial_text"].as_str().unwrap());
        let initial = if initial.is_null() { json!({}) } else { initial };
        let mut form = FilterForm::new(&initial);
        let details = form.details();
        if let Some(message) = case["constructor_error"].as_str() {
            assert_eq!(details.unwrap_err(), message, "{name}: the dialog fails to open with the oracle's text");
            errors += 1;
            continue;
        }
        let state = &case["state"];
        for (key, text) in state["fields"].as_object().unwrap() {
            assert_eq!(&form.fields[field_index(key).unwrap()], text.as_str().unwrap(), "{name}: field {key}");
        }
        assert_eq!(form.missing, state["missing"].as_bool().unwrap(), "{name}: missing");
        assert_eq!(form.virtual_only, state["virtual"].as_bool().unwrap(), "{name}: virtual");
        assert_eq!(form.orientation, state["orientation"].as_str().unwrap(), "{name}: orientation");
        assert_eq!(Value::Object(form.extra.clone()), state["extra"], "{name}: the rules it does not edit");
        assert_eq!(!form.extra.is_empty(), state["keep_complex_shown"].as_bool().unwrap(), "{name}: shown");
        assert_eq!(form.keep_complex, state["keep_complex"].as_bool().unwrap(), "{name}: kept");
        assert_eq!(details.unwrap(), state["details"].as_str().map(str::to_owned), "{name}: the description");
        for (key, value) in case["edits"].as_object().unwrap() {
            match (key.as_str(), value) {
                ("missing", v) => form.missing = v.as_bool().unwrap(),
                ("virtual", v) => form.virtual_only = v.as_bool().unwrap(),
                ("orientation", v) => form.orientation = v.as_str().unwrap().to_owned(),
                ("keep_complex", v) => form.keep_complex = v.as_bool().unwrap(),
                (key, v) => form.fields[field_index(key).unwrap()] = v.as_str().unwrap().to_owned(),
            }
        }
        match (form.rules(), case["error"].as_str()) {
            (Ok(rules), None) => {
                assert_eq!(dumps(&rules, true, false, false), case["rules_text"].as_str().unwrap(), "{name}: rules");
            }
            (Err(message), Some(expected)) => {
                assert_eq!(message, expected, "{name}: the box's text");
                errors += 1;
            }
            (got, expected) => panic!("{name}: got {got:?}, the oracle {expected:?}"),
        }
    }
    assert!(errors >= 15, "the error texts are covered ({errors})");
}

// ---- the listings -------------------------------------------------------------------------------------------

/// A [`Query`] from the generator's options (what `refresh_lists` passes to `library_query.query`).
fn query_of(options: &Value, root: &str) -> Query {
    let mut q = Query::default();
    if let Some(v) = options["rules"].as_object() {
        q.rules = Value::Object(v.clone());
    }
    q.collection = options["collection"].as_i64();
    q.search = options["search"].as_str().unwrap_or("").to_owned();
    q.mode = options["mode"].as_str().unwrap_or("all").to_owned();
    q.folder = options["folder"].as_str().map(|f| f.replace("{root}", root));
    q.sort = options["sort"].as_str().unwrap_or("imported").to_owned();
    q.collapsed = options["collapsed"].as_bool().unwrap_or(false);
    q
}

/// Whether a listing's options can be compared on this system. The golden library was written by the oracle on
/// Windows: its photos are at `Z:\GrainyGolden\...`, which on macOS is one file name in no folder (the oracle there
/// lists nothing for that folder either, and a `folder` rule reads `str(path.parent)`, `.`), so listings filtered
/// by a folder or a folder rule are compared on Windows only.
fn windows_folders(options: &Value) -> bool {
    cfg!(windows)
        || (options.get("folder").is_none_or(Value::is_null)
            && options.get("rules").and_then(|r| r.get("folder")).is_none())
}

fn listed(doc: &CatalogDocument) -> Vec<u64> {
    doc.photos().iter().map(|p| p.id).collect()
}

/// The list for every field of the dialog (and for the stored Lightroom searches and compound rules) is the
/// oracle's `library_query.query` list.
#[test]
fn every_filter_field_lists_what_the_oracle_lists() {
    let golden = Case::open("app", "collections_listings").json().clone();
    let (_dir, mut doc) = golden_library("collections-listings");
    let listings = golden["listings"].as_array().unwrap();
    assert!(listings.len() > 50);
    for listing in listings {
        let name = listing["name"].as_str().unwrap();
        if !windows_folders(&listing["options"]) {
            continue;
        }
        doc.set_query(query_of(&listing["options"], "")).unwrap();
        doc.refresh_now().unwrap();
        let expected: Vec<u64> = listing["ids"].as_array().unwrap().iter().map(|i| i.as_u64().unwrap()).collect();
        assert_eq!(listed(&doc), expected, "{name}");
        assert_eq!(doc.total() as u64, listing["total"].as_u64().unwrap(), "{name}: total");
    }
}

// ---- the sessions -------------------------------------------------------------------------------------------

/// The tables of `golden/app/collections_steps`, as the generator reads them.
fn tables(dir: &Path) -> Value {
    let db = Connection::open(dir.join("catalog.sqlite")).unwrap();
    let mut out = Map::new();
    for (table, columns) in [
        ("collections", vec!["id", "name", "parent_id", "rules"]),
        ("collection_members", vec!["collection_id", "photo_id"]),
        ("preferences", vec!["key", "value"]),
    ] {
        let list = columns.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(",");
        let mut statement = db.prepare(&format!("SELECT {list} FROM {table} ORDER BY {list}")).unwrap();
        let mut rows: Vec<Value> = statement
            .query_map([], |r| {
                let mut row = Map::new();
                for (i, name) in columns.iter().enumerate() {
                    let value = match r.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(v) => json!(v),
                        ValueRef::Real(v) => json!(v),
                        ValueRef::Text(t) => json!(String::from_utf8_lossy(t)),
                        ValueRef::Blob(_) => json!("blob"),
                    };
                    row.insert((*name).to_owned(), value);
                }
                Ok(Value::Object(row))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        if table == "preferences" {
            rows.retain(|r| r["key"] == "quick_collection");
        }
        out.insert(table.to_owned(), Value::Array(rows));
    }
    Value::Object(out)
}

/// The window stand-in's state: the selection and the open photo as the oracle's `selected_ids` reads them.
struct Session {
    cx: AppContext,
    dir: PathBuf,
    selection: Vec<u64>,
    current: Option<u64>,
    root: String,
}

impl Session {
    fn ids(&self) -> Vec<u64> {
        if self.selection.is_empty() {
            self.current.into_iter().collect()
        } else {
            self.selection.clone()
        }
    }

    fn settle(&mut self) {
        self.cx.doc.refresh().unwrap();
    }
}

fn change(cx: &mut AppContext, f: impl FnOnce(&mut Query)) {
    let mut q = cx.doc.query();
    f(&mut q);
    cx.doc.set_query(q).unwrap();
}

fn collections_of(cx: &AppContext) -> Vec<Collection> {
    cx.doc.collections()
}

fn id_of(value: &Value) -> Option<u64> {
    value.as_u64()
}

/// One step of a script. Returns what the step must have left, so the caller compares it with the golden.
fn run_step(s: &mut Session, step: &Value, golden: &Value) {
    let action = step[0].as_str().unwrap();
    let args = &step.as_array().unwrap()[1..];
    let at = format!("{step}");
    match action {
        "select" => s.selection = args[0].as_array().unwrap().iter().filter_map(id_of).collect(),
        "open" => s.current = args[0].as_u64(),
        "search" => change(&mut s.cx, |q| q.search = args[0].as_str().unwrap().to_owned()),
        "folder" => change(&mut s.cx, |q| q.folder = Some(args[0].as_str().unwrap().replace("{root}", &s.root))),
        "mode" => change(&mut s.cx, |q| q.mode = args[0].as_str().unwrap().to_owned()),
        "create" => {
            // The name box, then (smart) the rules box; a cancelled rules box makes nothing.
            let rules = &args[1];
            if rules != "cancel" {
                let ids = s.ids();
                coll::create_named(
                    &mut s.cx,
                    args[0].as_str().unwrap(),
                    (!rules.is_null()).then(|| rules.clone()),
                    ids,
                );
            }
        }
        "create_cancelled_name" => {}
        "add_to" => {
            let plain = coll::plain(&mut s.cx);
            match golden["error"].as_str() {
                Some(error) => {
                    assert!(plain.is_empty(), "{at}");
                    assert_eq!(error, "일반 컬렉션을 먼저 만들어 주세요.");
                    // The command shows the oracle's box.
                    s.cx.close_dialog("collections_error");
                    assert!(s.cx.run("collections.add"));
                    assert!(s.cx.dialog_open("collections_error"), "{at}");
                    s.cx.close_dialog("collections_error");
                }
                None => {
                    let items: Vec<&str> =
                        golden["boxes"][0]["items"].as_array().unwrap().iter().map(|i| i.as_str().unwrap()).collect();
                    assert_eq!(coll::choice_items(&plain), items, "{at}: the choices");
                    if let Some(index) = args[0].as_u64() {
                        let ids = s.ids();
                        coll::add_to(&mut s.cx, plain[index as usize].id, ids);
                    }
                }
            }
        }
        "pick" => coll::select(&mut s.cx, args[0].as_i64()),
        "pick_nothing" => coll::select(&mut s.cx, None),
        "remove_collection" => coll::remove_selected(&mut s.cx),
        "edit" => {
            let Some(id) = s.cx.doc.query().collection else {
                // Nothing is chosen: the command does nothing at all.
                assert!(s.cx.run("collections.edit"));
                assert!(s.cx.dialogs.is_empty(), "{at}");
                return;
            };
            let Ok(CollectionReply::Info { name, rules }) =
                s.cx.doc.collection_op(CollectionOp::Info { collection: id })
            else {
                panic!("{at}: the collection has no row")
            };
            let boxes = golden["boxes"].as_array().unwrap();
            assert_eq!(boxes[0]["text"].as_str().unwrap(), name, "{at}: the name box starts from the name");
            assert_eq!(boxes.len(), 1 + usize::from(rules.is_some() && !args[0].is_null()), "{at}: boxes");
            if let Some(new) = args[0].as_str() {
                if rules.is_some() {
                    assert_eq!(boxes[1]["rules"], rules.clone().unwrap(), "{at}: the rules box starts from the rules");
                    coll::edit_named(&mut s.cx, new, Some(args[1].clone()));
                } else {
                    coll::edit_named(&mut s.cx, new, None);
                }
            }
        }
        "remove_from" => {
            let ids = s.ids();
            coll::remove_from(&mut s.cx, ids);
        }
        "clear_filter" => coll::clear_filter(&mut s.cx),
        "filter" => {
            if !args[0].is_null() {
                assert_eq!(golden["boxes"][0]["rules"], s.cx.doc.query().rules, "{at}: the box starts from the rules");
                coll::apply_filter(&mut s.cx, args[0].clone());
            }
        }
        "quick_toggle" => {
            let ids = s.ids();
            coll::quick_toggle(&mut s.cx, &ids);
        }
        "quick_show" => {
            assert!(s.cx.run("collections.quick_show"));
        }
        "containing" => {
            let photo = s.current.unwrap();
            let found = coll::containing_of(&mut s.cx, photo);
            match golden["error"].as_str() {
                Some(error) => {
                    assert!(found.is_empty(), "{at}");
                    assert_eq!(error, "이 사진이 속한 컬렉션이 없습니다.");
                }
                None => {
                    let items: Vec<&str> =
                        golden["boxes"][0]["items"].as_array().unwrap().iter().map(|i| i.as_str().unwrap()).collect();
                    assert_eq!(coll::choice_items(&found), items, "{at}: the choices");
                    if let Some(index) = args.first().and_then(Value::as_u64) {
                        coll::show_collection(&mut s.cx, found[index as usize].id);
                    }
                }
            }
        }
        other => panic!("unknown step {other}"),
    }
}

fn run_session(name: &str, golden: &Value, script: &Value) {
    // The quick collection takes the interface language's name: Korean, as in the golden.
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let (dir, doc) = golden_library(&format!("collections-{name}"));
    let mut s = Session {
        cx: context(doc),
        dir,
        selection: Vec::new(),
        current: None,
        root: golden["folder_root"].as_str().unwrap().to_owned(),
    };
    let session = golden["sessions"].as_array().unwrap().iter().find(|x| x["name"] == name).unwrap();
    let script = script.as_array().unwrap();
    let mut last_tables = tables(&s.dir);
    for (step, expected) in script.iter().zip(session["steps"].as_array().unwrap()) {
        let at = format!("{name}: {step}");
        if !windows_folders(&json!({"folder": (step[0] == "folder").then_some(&step[1])})) {
            // From the folder filter on (`Z:\GrainyGolden\photos`, a Windows folder) every list is Windows-only.
            break;
        }
        s.cx.set_status("");
        run_step(&mut s, step, expected);
        let had_refresh = s.cx.doc.listing_pending();
        s.settle();
        let setup = ["search", "folder", "mode"].contains(&step[0].as_str().unwrap());
        if had_refresh && !setup {
            assert!(
                !expected["refreshes"].as_array().unwrap().is_empty(),
                "{at}: a refresh the oracle did not ask for"
            );
        }
        let q = s.cx.doc.query();
        assert_eq!(q.collection, expected["collection"].as_i64(), "{at}: collection");
        assert_eq!(q.rules, expected["rules"], "{at}: rules");
        assert_eq!(q.mode, expected["mode"].as_str().unwrap(), "{at}: mode");
        assert_eq!(q.folder.as_deref(), expected["folder"].as_str(), "{at}: folder");
        assert_eq!(q.search, expected["search"].as_str().unwrap(), "{at}: search");
        let ids: Vec<u64> = expected["ids"].as_array().unwrap().iter().filter_map(id_of).collect();
        assert_eq!(listed_ids(&s), ids, "{at}: the list");
        assert_eq!(s.cx.doc.total() as u64, expected["total"].as_u64().unwrap(), "{at}: total");
        // The tree and its current item.
        let tree = rows(&collections_of(&s.cx), &HashSet::new());
        let golden_tree: Vec<(String, i64, u64)> = expected["tree"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["text"].as_str().unwrap().to_owned(), r["id"].as_i64().unwrap(), r["depth"].as_u64().unwrap()))
            .collect();
        let ours: Vec<(String, i64, u64)> = tree.iter().map(|r| (r.text.clone(), r.id, r.depth as u64)).collect();
        assert_eq!(ours, golden_tree, "{at}: the tree");
        if let Some(message) = expected["messages"].get(0).and_then(Value::as_str) {
            assert_eq!(s.cx.status(), message, "{at}: status line");
        }
        if let Some(t) = expected.get("tables") {
            last_tables = t.clone();
        }
        assert_eq!(tables(&s.dir), last_tables, "{at}: tables");
    }
}

fn listed_ids(s: &Session) -> Vec<u64> {
    s.cx.doc.photos().iter().map(|p| p.id).collect()
}

#[test]
fn the_scripted_collection_session_matches_the_oracle() {
    let golden = Case::open("app", "collections_steps").json().clone();
    run_session("main", &golden, &parse(golden["scripts"]["main"].as_str().unwrap()));
}

#[test]
fn the_session_without_ordinary_collections_matches_the_oracle() {
    let golden = Case::open("app", "collections_steps").json().clone();
    run_session("no_plain", &golden, &parse(golden["scripts"]["no_plain"].as_str().unwrap()));
}

// ---- smaller behaviours -------------------------------------------------------------------------------------

/// The timed refresh (`refresh_timed_collection`) reads the list again only for relative date rules, in the
/// filter or in the chosen smart collection.
#[test]
fn the_timer_refreshes_only_relative_date_rules() {
    let _turn = common::language_lock();
    let (_dir, doc) = golden_library("collections-timer");
    let mut cx = context(doc);
    cx.doc.refresh().unwrap();
    coll::refresh_timed_collection(&mut cx);
    assert!(!cx.doc.listing_pending(), "no rules, no refresh");
    coll::apply_filter(&mut cx, json!({"rating_min": 3}));
    cx.doc.refresh().unwrap();
    coll::refresh_timed_collection(&mut cx);
    assert!(!cx.doc.listing_pending(), "plain rules");
    let recent =
        json!({"_adobe_smart": {"criteria": "captureTime", "operation": "inLast", "value": 7, "value_units": "days"}});
    coll::apply_filter(&mut cx, recent.clone());
    cx.doc.refresh().unwrap();
    coll::refresh_timed_collection(&mut cx);
    assert!(cx.doc.listing_pending(), "a relative date rule in the filter");
    // A refresh already asked for is not asked again over a running list.
    cx.doc.refresh().unwrap();
    coll::apply_filter(&mut cx, json!({}));
    cx.doc.refresh().unwrap();
    coll::create_named(&mut cx, "Recent", Some(recent), Vec::new());
    cx.doc.refresh().unwrap();
    assert!(!cx.doc.listing_pending());
    coll::refresh_timed_collection(&mut cx);
    assert!(cx.doc.listing_pending(), "a relative date rule in the chosen smart collection");
    cx.doc.refresh().unwrap();
    // The 60 second gate: the first call only starts the clock.
    cx.state::<CollectionsState>().timer = None;
    coll::timed_refresh(&mut cx);
    assert!(!cx.doc.listing_pending());
    cx.state::<CollectionsState>().timer = Some(Instant::now() - coll::TIMER - Duration::from_secs(1));
    coll::timed_refresh(&mut cx);
    assert!(cx.doc.listing_pending(), "after a minute");
}

/// A collection nests under the chosen one, a removed one leaves its children without a parent, and the tree
/// shows smart ones with the mark.
#[test]
fn the_tree_nests_and_marks_smart_collections() {
    let _turn = common::language_lock();
    let (_dir, doc) = golden_library("collections-tree");
    let mut cx = context(doc);
    coll::create_named(&mut cx, "Trip", None, vec![1]);
    coll::create_named(&mut cx, "Seoul", None, vec![2]);
    cx.doc.refresh().unwrap();
    let shown = |cx: &AppContext, collapsed: &HashSet<i64>| -> Vec<(String, usize)> {
        rows(&collections_of(cx), collapsed).iter().map(|r| (r.text.clone(), r.depth)).collect()
    };
    let expected = |names: &[(&str, usize)]| -> Vec<(String, usize)> {
        names.iter().map(|(n, d)| ((*n).to_owned(), *d)).collect()
    };
    assert_eq!(
        shown(&cx, &HashSet::new()),
        expected(&[("Trip", 0), ("Seoul", 1), ("여행", 0), ("◈ 좋은 사진", 0)]),
        "Seoul nests under Trip, which was chosen when it was made"
    );
    let trip = collections_of(&cx).iter().find(|c| c.name == "Trip").unwrap().id;
    assert_eq!(
        shown(&cx, &HashSet::from([trip])),
        expected(&[("Trip", 0), ("여행", 0), ("◈ 좋은 사진", 0)]),
        "a folded branch hides its children"
    );
    coll::select(&mut cx, Some(trip));
    coll::remove_selected(&mut cx);
    cx.doc.refresh().unwrap();
    assert_eq!(shown(&cx, &HashSet::new()), expected(&[("Seoul", 0), ("여행", 0), ("◈ 좋은 사진", 0)]));
    assert_eq!(cx.doc.query().collection, None, "the removed collection is no longer chosen");
}

/// Collections of a library opened read-only (the Python Grainy's own) cannot be changed; the commands that
/// would change them are off, the filter still works.
#[test]
fn a_read_only_library_keeps_its_collections() {
    let _turn = common::language_lock();
    let (dir, doc) = golden_library("collections-readonly");
    drop(doc);
    let scratch = common::temp_dir("collections-readonly-scratch");
    let doc = CatalogDocument::open_read_only(&dir, &scratch).unwrap();
    let mut cx = context(doc);
    cx.doc.refresh().unwrap();
    for id in
        ["collections.new", "collections.new_smart", "collections.add", "collections.delete", "cull.quick_collection"]
    {
        assert!(!grainy_app::commands::enabled(&cx, id), "{id} is off");
    }
    assert!(grainy_app::commands::enabled(&cx, "collections.filter"));
    assert!(grainy_app::commands::enabled(&cx, "collections.clear_filter"));
    let before = tables(&dir);
    coll::create_named(&mut cx, "Nope", None, vec![1]);
    assert!(cx.dialog_open("collections_error"), "the refusal is shown");
    assert_eq!(tables(&dir), before, "nothing was written");
    coll::apply_filter(&mut cx, json!({"rating_min": 5}));
    cx.doc.refresh().unwrap();
    assert_eq!(cx.doc.photos().iter().map(|p| p.id).collect::<Vec<_>>(), [6, 5]);
}

// ---- the window ---------------------------------------------------------------------------------------------

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

fn names(harness: &Harness<'_, GrainyApp>) -> Vec<String> {
    harness.state().cx.doc.collections().iter().map(|c| c.name.clone()).collect()
}

/// The fold-out, the boxes and the commands end to end on a library in a temporary folder.
fn window_flow(language: Language, tag: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let (mut harness, _dir) = library_harness(&format!("collections-window-{tag}"));
    let photo = harness.state().cx.doc.current_id();
    {
        let cx = &mut harness.state_mut().cx;
        coll::create_named(cx, "Trip", None, vec![photo]);
        coll::create_named(cx, "Seoul", None, vec![photo]);
        coll::select(cx, None);
        cx.doc.refresh().unwrap();
        let trip = cx.doc.collections().iter().find(|c| c.name == "Trip").unwrap().id;
        coll::select(cx, Some(trip));
        coll::create_named(cx, "Four stars", Some(json!({"rating_min": 4})), Vec::new());
        coll::select(cx, None);
        coll::quick_toggle(cx, &[photo]);
        cx.doc.refresh().unwrap();
        coll::select(cx, Some(trip));
    }
    harness.state_mut().open_foldout("collections");
    settle(&mut harness);
    assert_eq!(names(&harness).len(), 4, "Trip, Seoul, Four stars and the quick collection");
    save(&mut harness, &format!("collections-tree-{tag}"));

    // The filter box, empty and on a stored Lightroom search.
    assert!(harness.state_mut().run("collections.filter"));
    harness.run_steps(3);
    assert!(harness.state().dialog_open("filter"));
    save(&mut harness, &format!("collections-filter-{tag}"));
    harness.state_mut().close_dialog("filter");
    let complex = json!({"_adobe_smart": {"combine": "intersect", "children": [
        {"criteria": "rating", "operation": ">=", "value": 3.0},
        {"criteria": "captureTime", "operation": "inLast", "value": 7, "value_units": "days"}]}, "rating_min": 2});
    coll::apply_filter(&mut harness.state_mut().cx, complex);
    settle(&mut harness);
    assert!(harness.state_mut().run("collections.filter"));
    harness.run_steps(3);
    save(&mut harness, &format!("collections-filter-complex-{tag}"));
    harness.state_mut().close_dialog("filter");

    // The name box of the + button.
    assert!(harness.state_mut().run("collections.new"));
    harness.run_steps(3);
    assert!(harness.state().dialog_open("collections_name"));
    save(&mut harness, &format!("collections-name-{tag}"));
    harness.state_mut().close_dialog("collections_name");

    // 필터·컬렉션 해제 leaves nothing chosen and no rules.
    assert!(harness.state_mut().run("collections.clear_filter"));
    settle(&mut harness);
    let q = harness.state().cx.doc.query();
    assert_eq!((q.collection, q.rules.clone(), q.mode.as_str()), (None, json!({}), "all"));
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_window_shows_collections_and_filters_korean() {
    window_flow(Language::Korean, "ko");
}

#[test]
fn the_window_shows_collections_and_filters_english() {
    window_flow(Language::English, "en");
}
