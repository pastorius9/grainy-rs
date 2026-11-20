//! The library on disk against the oracle (`golden/app/library`, built by `tools/golden/gen_app.py` with the
//! Python Grainy's own code): the same photos in the same order for a spread of filters and sorts, the same
//! settings and history per photo, and after a scripted session of commits, undo, redo, ratings, labels, paste,
//! history restore and virtual copies through the Rust document, the same catalogue rows as the oracle's
//! `MainWindow` methods write for the same script. Also: importing a folder in place, undo across sessions,
//! removal that never touches files, and a read-only library that writes nothing.
//!
//! Every test works on copies in temporary folders; the Python Grainy's own folders are never opened.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use grainy_app::document::catalog::{import, CatalogDocument};
use grainy_app::document::{Document, LabelField, Query};
use grainy_catalog::rusqlite::{types::ValueRef, Connection};
use grainy_catalog::{Catalog, Sources};
use grainy_testkit::Case;
use serde_json::{json, Value};

fn case() -> Value {
    Case::open("app", "library").json().clone()
}

fn now() -> chrono::NaiveDateTime {
    chrono::NaiveDateTime::parse_from_str("2026-11-13 01:02:03", "%Y-%m-%d %H:%M:%S").unwrap()
}

/// A copy of the oracle's library in a fresh folder, opened with the oracle's clock and token.
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

fn query(options: &Value) -> Query {
    let mut q = Query::default();
    let o = options.as_object().unwrap();
    if let Some(v) = o.get("mode") {
        q.mode = v.as_str().unwrap().into();
    }
    if let Some(v) = o.get("search") {
        q.search = v.as_str().unwrap().into();
    }
    if let Some(v) = o.get("folder") {
        q.folder = v.as_str().map(str::to_owned);
    }
    if let Some(v) = o.get("recursive") {
        q.recursive = v.as_bool().unwrap();
    }
    if let Some(v) = o.get("collection") {
        q.collection = v.as_i64();
    }
    if let Some(v) = o.get("rules") {
        q.rules = v.clone();
    }
    if let Some(v) = o.get("sort") {
        q.sort = v.as_str().unwrap().into();
    }
    if let Some(v) = o.get("descending") {
        q.descending = v.as_bool().unwrap();
    }
    if let Some(v) = o.get("collapsed") {
        q.collapsed = v.as_bool().unwrap();
    }
    q
}

/// Whether a listing's options can be compared on this system. The golden library was written by the oracle on
/// Windows: its photos are at `Z:\GrainyGolden\...`, which on macOS is one file name in no folder (the oracle there
/// lists nothing for that folder either), so listings filtered by a folder are compared on Windows only.
fn windows_folders(options: &Value) -> bool {
    cfg!(windows) || options.get("folder").is_none_or(Value::is_null)
}

fn ids(doc: &dyn Document) -> Vec<u64> {
    doc.photos().iter().map(|p| p.id).collect()
}

fn open(doc: &mut CatalogDocument, id: u64) {
    if doc.index_of(id).is_none() {
        doc.set_query(Query::default()).unwrap();
        doc.refresh_now().unwrap();
    }
    let index = doc.index_of(id).unwrap_or_else(|| panic!("photo {id} listed"));
    doc.select(index);
    assert_eq!(doc.current_id(), id);
}

/// Every table of a catalogue, rows ordered by all columns, preview tokens replaced (as the generator dumps).
fn dump(path: &Path, token: &str) -> Value {
    let db = Connection::open(path).unwrap();
    let tables: Vec<String> = db
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut out = serde_json::Map::new();
    for table in tables {
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
                let mut row = serde_json::Map::new();
                for (i, column) in columns.iter().enumerate() {
                    let value = if table == "preview_state" && column == "token" {
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
        out.insert(table, Value::Array(rows));
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

#[test]
fn the_library_opens_as_the_oracle_wrote_it() {
    let meta = case();
    let (dir, doc) = golden_library("golden-start");
    equal(
        &dump(&dir.join("catalog.sqlite"), meta["token"].as_str().unwrap())["photos"],
        &meta["start_tables"]["photos"],
        "photos",
    );
    assert_eq!(doc.total(), 8);
}

#[test]
fn listings_match_the_oracle_for_filters_and_sorts() {
    let meta = case();
    let (_, mut doc) = golden_library("golden-listings");
    let listings = meta["listings"].as_array().unwrap();
    for listing in listings {
        let name = listing["name"].as_str().unwrap();
        if !windows_folders(&listing["options"]) {
            continue;
        }
        doc.set_query(query(&listing["options"])).unwrap();
        doc.refresh_now().unwrap();
        let expected: Vec<u64> = listing["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
        assert_eq!(ids(&doc), expected, "{name}");
        assert_eq!(doc.total() as u64, listing["total"].as_u64().unwrap(), "{name}: total");
        println!("{name}: {} photos in the oracle's order", expected.len());
    }
    // The folder tree counts what `library_query` counts per folder.
    doc.set_query(Query::default()).unwrap();
    doc.refresh_now().unwrap();
    let folders = doc.folders();
    let mut counts = Vec::new();
    fn walk(nodes: &[grainy_app::document::FolderNode], out: &mut Vec<(String, i64, i64)>) {
        for n in nodes {
            out.push((n.path.clone(), n.direct, n.total));
            walk(&n.children, out);
        }
    }
    walk(&folders, &mut counts);
    let root = meta["root"].as_str().unwrap();
    // `library_query`'s counts per folder (the virtual copy counts, as in the oracle); the folders are Windows
    // folders (see `windows_folders`).
    if cfg!(windows) {
        assert!(counts.contains(&(format!(r"{root}\photos"), 4, 8)), "{counts:?}");
        assert!(counts.contains(&(format!(r"{root}\photos\roll 2"), 3, 4)), "{counts:?}");
        assert!(counts.contains(&(format!(r"{root}\photos\roll 2\sub"), 1, 1)), "{counts:?}");
    }
    let collections = doc.collections();
    assert_eq!(
        collections.iter().map(|c| (c.name.as_str(), c.smart)).collect::<Vec<_>>(),
        [("여행", false), ("좋은 사진", true)]
    );
}

#[test]
fn settings_labels_and_history_match_the_oracle() {
    let meta = case();
    let (_, mut doc) = golden_library("golden-photos");
    for (id, expected) in meta["photos"].as_object().unwrap() {
        let id: u64 = id.parse().unwrap();
        open(&mut doc, id);
        let photo = doc.current().unwrap().clone();
        assert_eq!(photo.name, expected["name"].as_str().unwrap(), "{id}");
        assert_eq!(photo.rating, expected["rating"].as_i64().unwrap(), "{id}: rating");
        assert_eq!(photo.flag, expected["flag"].as_i64().unwrap(), "{id}: flag");
        assert_eq!(photo.label, expected["label"].as_str().unwrap_or(""), "{id}: label");
        assert_eq!(photo.stack, expected["stack_id"].as_i64(), "{id}: stack");
        assert_eq!(photo.virtual_source, expected["virtual_source"].as_u64(), "{id}: virtual source");
        let settings = Value::Object(doc.edit().settings().0.clone());
        assert!(grainy_catalog::json::equivalent(&settings, &expected["settings"]), "{id}: settings");
        let history: Vec<String> = doc.history().into_iter().map(|h| h.label).collect();
        let labels: Vec<String> =
            expected["history"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
        assert_eq!(history, labels, "{id}: history");
        let undo = expected["undo"]["undo"].as_array().map_or(0, Vec::len);
        let redo = expected["undo"]["redo"].as_array().map_or(0, Vec::len);
        assert_eq!(doc.edit().depth(), (undo, redo), "{id}: undo and redo across sessions");
    }
}

fn select_ids(doc: &mut CatalogDocument, ids: &[u64]) {
    let current = doc.current_index().unwrap();
    doc.select(current);
    for id in ids {
        if *id != doc.current_id() {
            let index = doc.index_of(*id).unwrap();
            doc.toggle_selected(index);
        }
    }
}

/// Runs one step of the generator's script through the document, as the window's commands do.
fn step(doc: &mut CatalogDocument, step: &Value) {
    let a = step.as_array().unwrap();
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
        "select" => {
            let ids: Vec<u64> = a[1].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
            select_ids(doc, &ids);
        }
        "rating" => doc.set_label_field(LabelField::Rating, a[1].clone()).unwrap(),
        "flag" => doc.set_label_field(LabelField::Flag, a[1].clone()).unwrap(),
        "label" => doc.set_label_field(LabelField::Label, a[1].clone()).unwrap(),
        "copy" => assert!(doc.copy()),
        "paste" => {
            let targets = doc.selected_indices();
            doc.paste(&targets).unwrap();
        }
        "restore_history" => assert!(doc.restore_history(a[1].as_u64().unwrap() as usize)),
        "virtual_copy" => {
            doc.virtual_copies().unwrap();
        }
        "reset" => doc.edit_mut().reset(),
        other => panic!("unknown step {other}"),
    }
    // The window stores what changed every frame.
    Document::flush(doc).unwrap();
}

#[test]
fn a_scripted_session_writes_what_the_oracle_writes() {
    let meta = case();
    let (dir, mut doc) = golden_library("golden-session");
    for (i, s) in meta["script"].as_array().unwrap().iter().enumerate() {
        step(&mut doc, s);
        println!("step {i}: {s}");
    }
    doc.edit_mut().commit("보정 변경");
    Document::flush(&mut doc).unwrap();
    doc.refresh_now().unwrap();
    let tables = dump(&dir.join("catalog.sqlite"), meta["token"].as_str().unwrap());
    for (table, expected) in meta["final_tables"].as_object().unwrap() {
        if !cfg!(windows) && table == "browser_index" {
            // Each photo's folder is the parent of its Windows path `Z:\GrainyGolden\...`; on macOS that path is
            // one file name, whose parent is `.` (as `Path(...).parent` in the oracle there): the other columns.
            let without = |rows: &Value| -> Value {
                rows.as_array()
                    .unwrap()
                    .iter()
                    .map(|r| {
                        let mut r = r.as_object().unwrap().clone();
                        r.remove("folder");
                        r.remove("folder_key");
                        Value::Object(r)
                    })
                    .collect()
            };
            equal(&without(&tables[table]), &without(expected), table);
            continue;
        }
        equal(&tables[table], expected, table);
        println!("{table}: {} rows equal to the oracle's", expected.as_array().map_or(0, Vec::len));
    }
    assert_eq!(tables.as_object().unwrap().len(), meta["final_tables"].as_object().unwrap().len());
}

fn file_states(files: &[PathBuf]) -> Vec<(Vec<u8>, std::time::SystemTime)> {
    files.iter().map(|f| (std::fs::read(f).unwrap(), std::fs::metadata(f).unwrap().modified().unwrap())).collect()
}

#[test]
fn import_in_place_thumbnails_undo_across_sessions_and_removal() {
    let dir = common::temp_dir("import");
    let photos = dir.join("photos");
    std::fs::create_dir_all(photos.join("sub")).unwrap();
    let files = common::eight_photos(&photos);
    std::fs::rename(&files[7], photos.join("sub").join("garden-08.png")).unwrap();
    let mut files: Vec<PathBuf> = files[..7].to_vec();
    files.push(photos.join("sub").join("garden-08.png"));
    let before = file_states(&files);
    let library = dir.join("Library");

    let mut doc = CatalogDocument::open(&library).unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let (found, roots) = import::scan(std::slice::from_ref(&photos), &cancel);
    assert_eq!(found.len(), 8);
    let report = import::import_files(doc.catalog(), &found, &roots, &cancel, &|_, _, _| {}).unwrap();
    assert_eq!(report.added.len(), 8, "{:?}", report.errors);
    // A second import adds nothing (`known` paths are skipped).
    let again = import::import_files(doc.catalog(), &found, &roots, &cancel, &|_, _, _| {}).unwrap();
    assert!(again.added.is_empty());
    doc.refresh_now().unwrap();
    assert_eq!(doc.len(), 8);
    assert_eq!(doc.current_index(), Some(0), "the first photo opens");
    // Thumbnails stored as the oracle stores them: valid for `preview_store.inspect`.
    for id in &report.added {
        let inspection = grainy_catalog::preview::inspect(&library, *id).unwrap();
        assert!(inspection.image.is_some(), "thumbnail of {id}");
        assert_eq!(inspection.state["rebuild"], false, "{id}: {}", inspection.state);
        assert_eq!(inspection.state["status"], "", "{id}");
    }
    // The folder registered and the tree.
    assert_eq!(doc.catalog().folder_roots().unwrap().len(), 1);
    assert_eq!(doc.folders().len(), 1);

    // An edit committed, then the library opened again: the edit and its undo come back.
    doc.edit_mut().set("exposure", 0.8);
    doc.edit_mut().commit("보정 변경");
    doc.set_label_field(LabelField::Rating, json!(5)).unwrap();
    Document::flush(&mut doc).unwrap();
    let first = doc.current_id();
    let stored = doc.catalog().photo(first as i64).unwrap().unwrap();
    assert_eq!(stored["settings"]["exposure"], json!(0.8));
    assert_eq!(stored["rating"], json!(5));
    // The cached thumbnail is stale now (its token changed).
    assert_eq!(grainy_catalog::preview::inspect(&library, first as i64).unwrap().state["rebuild"], true);
    drop(doc);
    let mut doc = CatalogDocument::open(&library).unwrap();
    doc.refresh_now().unwrap();
    assert_eq!(doc.current_id(), first);
    assert_eq!(doc.edit().settings().f64("exposure"), 0.8);
    assert_eq!(doc.history().first().map(|h| h.label.as_str()), Some("보정 변경 이전"));
    assert!(doc.edit_mut().undo(), "undo across sessions");
    Document::flush(&mut doc).unwrap();
    assert_eq!(doc.catalog().photo(first as i64).unwrap().unwrap()["settings"]["exposure"], json!(0.0));

    // Removal from the library: rows go, files stay.
    doc.select(1);
    let removed = doc.remove_selected().unwrap();
    assert_eq!(removed, 1);
    doc.refresh_now().unwrap();
    assert_eq!(doc.len(), 7);
    assert_eq!(file_states(&files), before, "original files are never written, moved or deleted");
}

#[test]
fn python_data_folders_are_rejected_before_settings_are_read_or_written() {
    // The guard compares these names before any filesystem access.
    let locations = grainy_app::paths::Locations::current();
    let dir = common::temp_dir("data-folder-guard");
    for folder in locations.oracle_folders() {
        let mut app = grainy_app::GrainyApp::with_library(
            &eframe::egui::Context::default(),
            &dir.join("unused-library"),
            Some(folder.clone()),
        );
        assert!(app.cx.data_folder().is_none());
        assert!(!dir.join("unused-library").exists());
        app.cx.set_data_folder(Some(folder));
        assert!(app.cx.data_folder().is_none());
    }
}

#[test]
fn a_read_only_snapshot_rejects_its_source_before_writing() {
    let dir = common::temp_dir("read-only-source-guard");
    let library = dir.join("Library");
    std::fs::create_dir_all(&library).unwrap();
    let catalog = library.join("catalog.sqlite");
    std::fs::copy(grainy_testkit::root().join("golden/app/library/start.sqlite"), &catalog).unwrap();
    let before = std::fs::read(&catalog).unwrap();
    for scratch in [library.clone(), library.join("new"), library.join("..").join("Library")] {
        assert!(CatalogDocument::open_read_only(&library, &scratch).is_err());
        assert_eq!(std::fs::read(&catalog).unwrap(), before);
    }
    assert!(!library.join("new").exists());
}

#[test]
fn a_read_only_library_writes_nothing() {
    let meta = case();
    let dir = common::temp_dir("read-only");
    let library = dir.join("Library");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::copy(grainy_testkit::root().join("golden/app/library/start.sqlite"), library.join("catalog.sqlite"))
        .unwrap();
    let hash = || std::fs::read(library.join("catalog.sqlite")).unwrap();
    let original = hash();
    let mut doc = CatalogDocument::open_read_only(&library, &dir.join("scratch")).unwrap();
    doc.set_clock(Arc::new(now));
    doc.refresh_now().unwrap();
    assert!(doc.read_only());
    let expected: Vec<u64> =
        meta["listings"][0]["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
    assert_eq!(ids(&doc), expected);
    // Edits, undo, labels and a paste stay in memory.
    doc.edit_mut().set("exposure", 1.0);
    doc.edit_mut().commit("보정 변경");
    doc.set_label_field(LabelField::Rating, json!(1)).unwrap();
    assert!(doc.copy());
    doc.select(1);
    doc.paste(&[1]).unwrap();
    Document::flush(&mut doc).unwrap();
    assert_eq!(doc.edit().settings().f64("exposure"), 1.0, "pasted in memory");
    doc.select(0);
    assert_eq!(doc.edit().settings().f64("exposure"), 1.0, "kept in memory");
    assert!(doc.edit_mut().undo());
    Document::flush(&mut doc).unwrap();
    assert!(doc.remove_selected().is_err());
    assert!(doc.virtual_copies().is_err());
    assert!(doc.add_files(vec![], &[]).is_err());
    doc.refresh_now().unwrap();
    assert_eq!(doc.photos()[0].rating, 1, "labels stay in memory across listings");
    drop(doc);
    assert!(hash() == original, "the catalogue file is unchanged");
    let names: Vec<String> =
        std::fs::read_dir(&library).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["catalog.sqlite"], "nothing was added to the library folder");
}
