//! Work package 1: the library column and the saved session, against the oracle (`golden/app/libcol_*`, built by
//! `tools/golden/gen_app.py` with the Python Grainy's own `FolderPanel.remove`, `save_folder_state`,
//! `change_sort`, `DevelopPanels.save`, `FolderPanel.nodes` and `import_reason`).
//!
//! The model tests drive the context without a window (`session_state::poll` is what the window runs every
//! frame); the window tests render the real application headless and write `tests/snapshots/library-column-*`.
//! Every test works on copies and generated photos in temporary folders.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::library_column::{import_done, remove_now, LAUNCH_DISABLED};
use grainy_app::commands::Invocation;
use grainy_app::document::catalog::{import, CatalogDocument};
use grainy_app::document::Document;
use grainy_app::render::TestCard;
use grainy_app::session_state::{self, ColumnState, LibraryLock, TreeNode, ALREADY_OPEN};
use grainy_app::{AppContext, GrainyApp, Mode};
use grainy_catalog::folders::Platform;
#[cfg(windows)]
use grainy_catalog::rusqlite::{types::ValueRef, Connection};
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Value};

// ---- the oracle's library -----------------------------------------------------------------------------------

fn golden_library(name: &str) -> (PathBuf, CatalogDocument) {
    let meta = Case::open("app", "library").json().clone();
    let dir = common::temp_dir(name);
    std::fs::copy(grainy_testkit::root().join("golden/app/library/start.sqlite"), dir.join("catalog.sqlite"))
        .expect("copy the library");
    let stamp = meta["stamp"].as_str().unwrap().to_owned();
    let token = meta["token"].as_str().unwrap().to_owned();
    let catalog = Catalog::with_sources(&dir, Sources::new(move || stamp.clone(), move || token.clone())).unwrap();
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    (dir, doc)
}

fn context(doc: CatalogDocument) -> AppContext {
    AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc))
}

/// The tables of `golden/app/libcol_*` (`columns`), rows ordered by all their columns.
#[cfg(windows)]
fn projected(path: &Path, columns: &Value) -> Value {
    let db = Connection::open(path).unwrap();
    let mut out = serde_json::Map::new();
    for (table, names) in columns.as_object().unwrap() {
        let names: Vec<&str> = names.as_array().unwrap().iter().map(|n| n.as_str().unwrap()).collect();
        let list = names.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(",");
        let mut statement = db.prepare(&format!("SELECT {list} FROM {table} ORDER BY {list}")).unwrap();
        let rows: Vec<Value> = statement
            .query_map([], |r| {
                let mut row = serde_json::Map::new();
                for (i, name) in names.iter().enumerate() {
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
        out.insert(table.clone(), Value::Array(rows));
    }
    Value::Object(out)
}

/// A table of a projection without the `folder_panel` preference (that one is compared on its own).
#[cfg(windows)]
fn without_folder_panel(mut tables: Value) -> Value {
    if let Some(rows) = tables["preferences"].as_array_mut() {
        rows.retain(|r| r["key"] != "folder_panel");
    }
    tables
}

#[cfg(windows)]
fn preference(rows: &Value, key: &str) -> Option<Value> {
    let row = rows["preferences"].as_array()?.iter().find(|r| r["key"] == key)?;
    serde_json::from_str(row["value"].as_str()?).ok()
}

/// What the document stores: the list is read again first.
fn settle_model(cx: &mut AppContext) {
    cx.doc.refresh().unwrap();
    session_state::poll(cx);
}

/// The oracle's script sets the expanded folders itself (new folders with photos otherwise expand by themselves
/// while the saved state has no `expanded`).
#[cfg(windows)]
fn set_expanded<'a>(cx: &mut AppContext, keys: impl IntoIterator<Item = &'a str>) {
    let st = cx.state::<ColumnState>();
    st.expanded.clear();
    st.expanded.extend(keys.into_iter().map(str::to_owned));
}

fn open_photo(cx: &mut AppContext, id: u64) {
    let index = cx.doc.index_of(id).unwrap_or_else(|| panic!("photo {id} is listed"));
    cx.select_photo(index);
}

#[cfg(windows)]
fn choose_folder(cx: &mut AppContext, folder: Option<&str>) {
    let mut query = cx.doc.query();
    query.folder = folder.map(str::to_owned);
    cx.doc.set_query(query).unwrap();
    cx.doc.refresh().unwrap();
}

// ---- removing a folder ------------------------------------------------------------------------------------

// The golden library is the oracle's, written on Windows: its folders are Windows folders (`Z:\GrainyGolden\...`,
// the drive `Z:\`), which macOS reads as plain file names in no folder; the folder steps compare on Windows only.

/// `FolderPanel.remove` through the − button's command: the question, the backup, the catalogue's tables and
/// the saved state equal what the oracle's own methods leave.
#[cfg(windows)]
#[test]
fn removing_a_folder_matches_the_oracle() {
    let _turn = common::language_lock();
    let g = Case::open("app", "libcol_remove").json().clone();
    let (dir, doc) = golden_library("libcol-remove");
    let mut cx = context(doc);
    session_state::poll(&mut cx);
    let folder = g["folder"].as_str().unwrap().to_owned();
    choose_folder(&mut cx, Some(&folder));
    open_photo(&mut cx, g["open_photo"].as_u64().unwrap());
    set_expanded(&mut cx, g["expanded"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()));
    settle_model(&mut cx);
    let catalog_file = dir.join("catalog.sqlite");
    assert_eq!(projected(&catalog_file, &g["columns"]), g["before"], "the saved state before the removal");

    // The − button: a question with the oracle's text; nothing is removed before 예.
    assert!(cx.run("library.remove_folder"));
    assert!(cx.dialog_open("remove_folder"));
    assert!(!dir.join("backups").exists(), "no backup before the answer");
    cx.close_dialog("remove_folder");
    remove_now(&mut cx, &folder);
    settle_model(&mut cx);

    let backups: Vec<_> = std::fs::read_dir(dir.join("backups"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("folder-removal-"))
        .collect();
    assert_eq!(backups.len(), g["backups"].as_u64().unwrap() as usize);
    assert_eq!(projected(&backups[0].path(), &g["columns"]), g["backup"], "the backup is the catalogue before");

    let after = projected(&catalog_file, &g["columns"]);
    assert_eq!(without_folder_panel(after.clone()), without_folder_panel(g["final"].clone()));
    assert!(cx.doc.query().folder.is_none() && g["folder_filter_after"].is_null());
    let mut expanded: Vec<String> = cx.state::<ColumnState>().expanded.iter().cloned().collect();
    expanded.sort();
    assert_eq!(json!(expanded), g["expanded_after"]);
    // The oracle's stand-in does not read the list again, so its `current_photo` is empty; the real window
    // opens the first photo (`ensure_current`) and the state follows it.
    let saved = preference(&after, "folder_panel").expect("folder_panel saved");
    let mut oracle = preference(&g["final"], "folder_panel").expect("oracle's folder_panel");
    oracle["current_photo"] = json!(cx.doc.current_id());
    assert_eq!(saved, oracle);
    // Files are never touched: the catalogue holds paths of a folder that does not exist here.
    assert!(!folder.is_empty());
}

#[cfg(windows)]
#[test]
fn removing_everything_and_refusing_a_drive_match_the_oracle() {
    let _turn = common::language_lock();
    let g = Case::open("app", "libcol_remove_all").json().clone();
    let (dir, doc) = golden_library("libcol-remove-all");
    let mut cx = context(doc);
    session_state::poll(&mut cx);
    let folder = g["folder"].as_str().unwrap().to_owned();
    choose_folder(&mut cx, Some(&folder));
    set_expanded(&mut cx, [Platform::native().path_key(&folder).as_str()]);
    settle_model(&mut cx);
    // A drive root is refused without a question or a backup.
    cx.run_with("library.remove_folder", &Invocation::value("Z:\\"));
    assert!(!cx.dialog_open("remove_folder"));
    let backups = |dir: &Path| std::fs::read_dir(dir.join("backups")).map_or(0, |d| d.flatten().count());
    assert_eq!(backups(&dir), g["backups_after_drive"].as_u64().unwrap() as usize);
    remove_now(&mut cx, &folder);
    settle_model(&mut cx);
    assert_eq!(backups(&dir), g["backups"].as_u64().unwrap() as usize);
    let after = projected(&dir.join("catalog.sqlite"), &g["columns"]);
    assert_eq!(without_folder_panel(after.clone()), without_folder_panel(g["final"].clone()));
    assert!(cx.doc.is_empty() && cx.doc.current().is_none());
    assert!(cx.doc.query().folder.is_none());
    assert!(cx.state::<ColumnState>().expanded.is_empty());
}

// ---- the saved state --------------------------------------------------------------------------------------

/// `save_folder_state`, `change_sort` and `DevelopPanels.save`, step by step: after each step of the oracle's
/// script the three preferences hold the oracle's values (and nothing else is written).
#[cfg(windows)]
#[test]
fn the_saved_state_follows_the_oracle_step_by_step() {
    let _turn = common::language_lock();
    let g = Case::open("app", "libcol_state").json().clone();
    let (_, doc) = golden_library("libcol-state");
    let mut cx = context(doc);
    session_state::poll(&mut cx);
    let roll2 = format!(r"{}\photos\roll 2", g["root"].as_str().unwrap());
    let photos = format!(r"{}\photos", g["root"].as_str().unwrap());
    let platform = Platform::native();
    for step in g["steps"].as_array().unwrap() {
        match step["step"].as_str().unwrap() {
            "select_folder" => {
                choose_folder(&mut cx, Some(&roll2));
                open_photo(&mut cx, 6);
                set_expanded(&mut cx, [platform.path_key(&photos).as_str(), platform.path_key(&roll2).as_str()]);
            }
            "subfolders_off" => {
                let mut q = cx.doc.query();
                q.recursive = false;
                cx.doc.set_query(q).unwrap();
            }
            "sort_name_descending" => {
                let mut q = cx.doc.query();
                q.sort = "name".into();
                q.descending = true;
                cx.doc.set_query(q).unwrap();
            }
            "panels_curve_open" => {
                cx.set_solo(false);
                cx.open_panel("curve");
            }
            "all_photos" => {
                choose_folder(&mut cx, None);
                open_photo(&mut cx, 3);
                cx.state::<ColumnState>().expanded.retain(|k| *k == platform.path_key(&photos));
            }
            "panels_solo_curve_only" => {
                cx.set_solo(true);
                cx.open_panel("curve");
                cx.set_solo(true);
            }
            other => panic!("{other}"),
        }
        settle_model(&mut cx);
        for key in ["folder_panel", "browser_view", "develop_panels"] {
            let stored = cx.doc.preference(key);
            let expected = step["values"][key].clone();
            let expected = if expected.is_null() { None } else { Some(expected) };
            assert_eq!(stored, expected, "{} after {}", key, step["step"]);
        }
    }
}

/// A library reopened: the folder, the subfolder box, the expanded folders, the open photo, the sort order, the
/// open panels and the screen come back (`restore_folder_selection`, `browser_preferences`, `finish`).
#[test]
fn a_reopened_library_restores_its_session() {
    let _turn = common::language_lock();
    let dir = common::temp_dir("libcol-reopen");
    let photos = dir.join("Shots");
    std::fs::create_dir_all(&photos).unwrap();
    let files = common::eight_photos(&photos);
    let library = dir.join("Library");
    let (folder, photo) = {
        let mut doc = CatalogDocument::open(&library).unwrap();
        doc.add_files(files.clone(), std::slice::from_ref(&photos)).unwrap();
        doc.refresh_now().unwrap();
        let mut cx = context(doc);
        session_state::poll(&mut cx);
        settle_model(&mut cx);
        let shots = find_node(&cx.state::<ColumnState>().nodes, "Shots").expect("the imported folder is listed");
        let platform = Platform::native();
        let mut q = cx.doc.query();
        q.folder = Some(shots.path.clone());
        q.sort = "rating".into();
        q.descending = true;
        q.recursive = false;
        cx.doc.set_query(q).unwrap();
        cx.doc.refresh().unwrap();
        open_photo(&mut cx, 3);
        cx.state::<ColumnState>().expanded.insert(platform.path_key(&shots.path));
        cx.set_solo(false);
        cx.open_panel("curve");
        settle_model(&mut cx);
        (shots.path, cx.doc.current_id())
    };
    assert_eq!(photo, 3);

    let doc = CatalogDocument::open(&library).unwrap();
    let mut cx = context(doc);
    session_state::poll(&mut cx);
    cx.doc.refresh().unwrap();
    session_state::poll(&mut cx);
    let query = cx.doc.query();
    assert_eq!(query.folder.as_deref(), Some(folder.as_str()));
    assert!(!query.recursive);
    assert_eq!((query.sort.as_str(), query.descending), ("rating", true));
    assert_eq!(cx.doc.current_id(), photo, "the photo that was open opens again");
    assert_eq!(cx.mode(), Mode::Library, "a saved folder brings the library screen");
    assert!(cx.state::<ColumnState>().expanded.contains(&Platform::native().path_key(&folder)));
    assert_eq!(cx.expanded_panels(), ["basic", "curve"]);
    // Nothing changed, so nothing is written again.
    let stored = cx.doc.preference("folder_panel").unwrap();
    assert_eq!(stored["current_photo"], json!(photo));
}

fn find_node(nodes: &[TreeNode], name: &str) -> Option<TreeNode> {
    nodes.iter().find_map(|n| if n.name == name { Some(n.clone()) } else { find_node(&n.children, name) })
}

/// `DevelopPanels.finish`: solo keeps the first open panel; tool panels and unknown keys are dropped; an
/// invalid sort falls back to the first entry.
#[test]
fn saved_panels_and_sort_are_normalised_as_the_oracle_does() {
    let _turn = common::language_lock();
    let (_, mut doc) = golden_library("libcol-normalise");
    doc.save_preference("develop_panels", &json!({"solo": true, "expanded": ["crop", "film", "curve", "nope"]}))
        .unwrap();
    doc.save_preference("browser_view", &json!({"sort": "nonsense", "descending": true})).unwrap();
    let mut cx = context(doc);
    session_state::poll(&mut cx);
    assert_eq!(cx.expanded_panels(), ["film"]);
    assert_eq!(cx.doc.query().sort, "imported");
    assert!(cx.doc.query().descending);
    // Not solo: every listed panel opens, in the saved order.
    let (_, mut doc) = golden_library("libcol-normalise-2");
    doc.save_preference("develop_panels", &json!({"solo": false, "expanded": ["curve", "masks", "basic"]})).unwrap();
    let mut cx = context(doc);
    session_state::poll(&mut cx);
    assert_eq!(cx.expanded_panels(), ["curve", "basic"]);
}

// ---- the folder tree --------------------------------------------------------------------------------------

/// `FolderPanel.nodes`: the same flat tree, with the missing and unscanned marks, scanned subfolders under
/// registered roots, and folders that were removed left out.
#[cfg(windows)]
#[test]
fn the_folder_tree_matches_the_oracle() {
    let g = Case::open("app", "libcol_tree").json().clone();
    let rows = g["rows"].as_array().unwrap().clone();
    let roots: Vec<String> = g["roots"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    for case in g["cases"].as_array().unwrap() {
        let strings = |k: &str| -> Vec<String> {
            case[k].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect()
        };
        let missing = strings("missing").into_iter().collect();
        let scanned = strings("scanned").into_iter().collect();
        let nodes =
            session_state::flat_nodes(&rows, &roots, &strings("removed"), &strings("children"), &missing, &scanned);
        let expected = case["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), expected.len(), "{}", case["name"]);
        for ((parent, node), want) in nodes.iter().zip(expected) {
            let name = &case["name"];
            assert_eq!(node.key, want["key"].as_str().unwrap(), "{name}");
            assert_eq!(parent.as_deref(), want["parent"].as_str(), "{name} {}", node.key);
            assert_eq!(node.path, want["path"].as_str().unwrap(), "{name}");
            assert_eq!(node.name, want["name"].as_str().unwrap(), "{name}");
            assert_eq!(node.drive, want["drive"].as_bool().unwrap(), "{name}");
            assert_eq!((node.direct, node.total), (want["direct"].as_i64().unwrap(), want["total"].as_i64().unwrap()));
            assert_eq!(node.missing, want["missing"].as_bool().unwrap(), "{name} {}", node.key);
            assert_eq!(node.unscanned, want["unscanned"].as_bool().unwrap(), "{name} {}", node.key);
        }
    }
}

// ---- import: reasons, cancel, the box ---------------------------------------------------------------------

#[test]
fn import_reasons_match_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let g = Case::open("app", "libcol_import_reason").json().clone();
    for case in g["cases"].as_array().unwrap() {
        let reason =
            session_state::import_reason(Path::new(case["path"].as_str().unwrap()), case["error"].as_str().unwrap());
        assert_eq!(reason, case["reason"].as_str().unwrap(), "{}", case["path"]);
    }
}

/// `cancel_import`: the import stops before the next file, photos already added stay, and the status says so.
#[test]
fn cancelling_an_import_keeps_what_was_added() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("libcol-cancel");
    let shots = dir.join("Shots");
    std::fs::create_dir_all(&shots).unwrap();
    common::eight_photos(&shots);
    let catalog = Catalog::new(dir.join("Library")).unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let (files, roots) = import::scan(std::slice::from_ref(&shots), &cancel);
    assert_eq!(files.len(), 8);
    let seen = std::cell::Cell::new(0);
    let report = import::import_files(&catalog, &files, &roots, &cancel, &|done, _, _| {
        seen.set(done);
        if done == 3 {
            cancel.store(true, Ordering::Relaxed);
        }
    })
    .unwrap();
    // The cancel came while the fourth file was being read: that file is finished, no further one starts.
    assert_eq!(report.added.len(), 4, "the photos before the cancel stay");
    assert_eq!(catalog.paths().unwrap().len(), 4);

    let (_, doc) = golden_library("libcol-cancel-status");
    let mut cx = context(doc);
    cx.state::<ColumnState>().cancelling = true;
    import_done(&mut cx, Ok(report));
    assert_eq!(cx.status(), grainy_i18n::tr("가져오기를 중단했습니다. 이미 가져온 사진은 유지합니다."));
    assert!(!cx.state::<ColumnState>().cancelling && !cx.dialog_open("import_errors"));
}

/// The 읽을 수 없는 사진 box lists the reasons (the first ten, one per line).
#[test]
fn unreadable_photos_are_listed_in_a_box() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let (_, doc) = golden_library("libcol-errors");
    let mut cx = context(doc);
    let errors: Vec<String> = (1..=12).map(|i| format!("bad-{i}.jpg: reason {i}")).collect();
    let report = import::Report { added: vec![], errors: errors.clone() };
    import_done(&mut cx, Ok(report));
    assert!(cx.dialog_open("import_errors"));
    assert_eq!(cx.status(), grainy_i18n::tr("사진 가져오기 완료"));
    let notice = grainy_app::dialogs::import_errors::Notice::import_errors(&errors);
    assert_eq!(notice.text.lines().count(), 10);
    assert_eq!(notice.title, grainy_i18n::tr("읽을 수 없는 사진"));
}

// ---- the library lock -------------------------------------------------------------------------------------

#[test]
fn a_second_window_on_the_same_library_is_refused() {
    let dir = common::temp_dir("libcol-lock").join("Library");
    let first = LibraryLock::acquire(&dir).expect("the first window locks");
    let second = LibraryLock::acquire(&dir).expect_err("the second window is refused");
    assert_eq!(second, ALREADY_OPEN);
    assert_eq!(ALREADY_OPEN, "This photo library is already open.\n이 사진 라이브러리가 이미 열려 있습니다.");
    drop(first);
    LibraryLock::acquire(&dir).expect("the lock is free again after the window closed");
    // Another library is not affected.
    let other = common::temp_dir("libcol-lock-other").join("Library");
    let _a = LibraryLock::acquire(&other).unwrap();
}

// ---- windows -----------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, Ordering::Relaxed);
    Harness::builder().with_size(egui::vec2(1600.0, 1000.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

fn library_harness<'a>(library: &Path, setup: impl FnOnce(&mut GrainyApp) + 'a) -> Harness<'a, GrainyApp> {
    let library = library.to_path_buf();
    builder().wgpu().build_eframe(move |cc| {
        let doc = CatalogDocument::open(&library).expect("library");
        let mut app =
            GrainyApp::with_document(&cc.egui_ctx, Box::new(grainy_app::worker::Worker::new()), Box::new(doc));
        setup(&mut app);
        app
    })
}

fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle(harness);
}

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) {
    common::snapshot::save(harness, name);
}

/// Photos in fixed folders (their names are on screen): Trip with two days, Archive, Gone.
fn shoot(dir: &Path) -> Vec<PathBuf> {
    let source = dir.join("source");
    std::fs::create_dir_all(&source).unwrap();
    let eight = common::eight_photos(&source);
    let place = [("Trip/Day 1", 0..3), ("Trip/Day 2", 3..5), ("Archive", 5..7), ("Gone", 7..8)];
    for (name, range) in place {
        let folder = dir.join("Pictures").join(name);
        std::fs::create_dir_all(&folder).unwrap();
        for file in &eight[range] {
            std::fs::copy(file, folder.join(file.file_name().unwrap())).unwrap();
        }
    }
    vec![dir.join("Pictures").join("Trip"), dir.join("Pictures").join("Archive"), dir.join("Pictures").join("Gone")]
}

/// A library with a folder that is gone: the tree marks it with `? ` and the counts follow the subfolder box.
fn tree_snapshot(language: Language, name: &str, scene: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let dir = common::temp_dir(scene);
    let roots = shoot(&dir);
    let library = dir.join("Library");
    let mut harness = library_harness(&library, |app| {
        app.open_paths(roots);
        app.set_mode(Mode::Library);
    });
    settle(&mut harness);
    assert_eq!(harness.state().document().len(), 8, "{}", harness.state().status());
    // The folder goes away on disk (renamed): the next scan marks it missing; the photos stay in the catalogue.
    std::fs::rename(dir.join("Pictures").join("Gone"), dir.join("Elsewhere")).unwrap();
    harness.state_mut().run("library.refresh_folders");
    settle(&mut harness);
    let nodes = harness.state_mut().state::<ColumnState>().nodes.clone();
    let gone = find_node(&nodes, "Gone").expect("the folder stays in the tree");
    assert!(gone.missing, "a folder that is gone is marked");
    let trip = find_node(&nodes, "Trip").expect("Trip");
    assert!(!trip.missing && (trip.direct, trip.total) == (0, 5), "{trip:?}");
    // Day 1 is chosen; the box 하위 폴더 사진 포함 stays on.
    let day = find_node(&nodes, "Day 1").expect("Day 1");
    let mut query = harness.state().document().query();
    query.folder = Some(day.path.clone());
    harness.state_mut().document_mut().set_query(query).unwrap();
    settle(&mut harness);
    assert_eq!(harness.state().document().len(), 3);
    // Which folders opened by themselves depends on when the import's listing grew: open exactly Trip.
    common::snapshot::open_folders(harness.state_mut(), &["Trip"]);
    settle(&mut harness);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn folder_tree_with_a_missing_folder_korean() {
    tree_snapshot(Language::Korean, "library-column-tree-ko", "libcol-tree-ko");
}

#[test]
fn folder_tree_with_a_missing_folder_english() {
    tree_snapshot(Language::English, "library-column-tree-en", "libcol-tree-en");
}

/// The folder commands of the context menu on a real library.
#[test]
fn folder_commands() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    LAUNCH_DISABLED.store(true, Ordering::Relaxed);
    let dir = common::temp_dir("libcol-commands");
    let roots = shoot(&dir);
    let library = dir.join("Library");
    let mut harness = library_harness(&library, |app| app.open_paths(roots));
    settle(&mut harness);
    let nodes = harness.state_mut().state::<ColumnState>().nodes.clone();
    let day2 = find_node(&nodes, "Day 2").expect("Day 2");
    let app = harness.state_mut();

    // 탐색기에서 열기 starts nothing here; 동기화 of a folder that is gone says so.
    assert!(app.run_with("library.open_folder", &Invocation::value(day2.path.clone())));
    assert!(app.run_with("library.sync_folder", &Invocation::value(dir.join("nowhere").to_string_lossy().to_string())));
    assert_eq!(app.status(), grainy_i18n::tr("폴더를 찾을 수 없습니다. 드라이브 연결이나 원본 위치를 확인해 주세요."));

    // 폴더 동기화 of a present folder imports it again: nothing new, nothing duplicated.
    assert!(app.run_with("library.sync_folder", &Invocation::value(day2.path.clone())));
    assert!(app.jobs.running("import"));
    settle(&mut harness);
    assert_eq!(harness.state().document().total(), 8);

    // 상위 폴더 표시: the parent of Trip becomes a registered folder, expanded.
    let trip = find_node(&nodes, "Trip").expect("Trip");
    let parent = Platform::native().parent(&trip.path);
    assert!(harness.state_mut().run_with("library.show_parent", &Invocation::value(trip.path.clone())));
    settle(&mut harness);
    let (_, inputs) = harness.state().document().folder_inputs();
    assert!(inputs.roots.iter().any(|r| Platform::native().path_key(r) == Platform::native().path_key(&parent)));
    assert!(harness.state_mut().state::<ColumnState>().expanded.contains(&Platform::native().path_key(&parent)));

    // The folder's photos leave the catalogue; the files stay.
    let before = std::fs::read_dir(&day2.path).unwrap().count();
    harness.state_mut().run_with("library.remove_folder", &Invocation::value(day2.path.clone()));
    assert!(harness.state().dialog_open("remove_folder"));
    // 예 (the question is the shared Confirm dialog; its action is what `remove_now` does).
    harness.state_mut().close_dialog("remove_folder");
    remove_now(harness.state_mut(), &day2.path);
    settle(&mut harness);
    assert_eq!(harness.state().document().total(), 6);
    assert_eq!(std::fs::read_dir(&day2.path).unwrap().count(), before, "the files are never touched");
    assert!(find_node(&harness.state_mut().state::<ColumnState>().nodes, "Day 2").is_none());
    LAUNCH_DISABLED.store(false, Ordering::Relaxed);
}

/// While an import runs the list grows (`import_refresh_timer`), the status bar offers 가져오기 중단, and a
/// file that cannot be read ends in the box with the oracle's wording of the reason.
#[test]
fn the_list_grows_during_an_import_and_unreadable_photos_are_listed() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("libcol-grow");
    let shots = dir.join("Shots");
    std::fs::create_dir_all(&shots).unwrap();
    common::eight_photos(&shots);
    std::fs::write(shots.join("broken.jpg"), b"this is not a picture").unwrap();
    std::fs::write(shots.join("junk.dng"), b"not a raw file either").unwrap();
    let library = dir.join("Library");
    let mut harness = library_harness(&library, |app| {
        app.open_paths(vec![shots.clone()]);
        app.set_mode(Mode::Library);
    });
    let started = Instant::now();
    let (mut sizes, mut offered) = (Vec::new(), false);
    // The status line shows the message that came last: the import's 사진 가져오기 완료, or the size line of the
    // open photo's preview, which is rendered by another job and may arrive after it (on a slow machine it does).
    // The import's message is looked for in every frame.
    let complete = grainy_i18n::tr("사진 가져오기 완료");
    let mut said_complete = false;
    while !(harness.state().idle() && !sizes.is_empty()) {
        harness.step();
        offered |= harness.state().jobs.running("import")
            && grainy_app::commands::enabled(harness.state(), "library.cancel_import");
        said_complete |= harness.state().status() == complete;
        sizes.push(harness.state().document().len());
        std::thread::sleep(Duration::from_millis(5));
        assert!(started.elapsed() < Duration::from_secs(300));
    }
    settle(&mut harness);
    assert!(sizes.windows(2).all(|w| w[0] <= w[1]), "the list only grows: {sizes:?}");
    assert_eq!(*sizes.last().unwrap(), 8);
    assert!(offered, "the cancel button shows while the import runs");
    assert!(!grainy_app::commands::enabled(harness.state(), "library.cancel_import"), "and goes when it is over");
    let app = harness.state();
    assert!(app.dialog_open("import_errors"), "{}", app.status());
    assert!(
        said_complete || app.status() == complete,
        "the import said {complete:?}; the status is {:?}",
        app.status()
    );
    assert_eq!(app.document().total(), 8);
    common::snapshot::open_folders(harness.state_mut(), &[]);
    // The picture shows the import's message whichever job answered last.
    common::snapshot::pin_status(harness.state_mut(), &complete);
    harness.run_steps(3);
    save(&mut harness, "library-column-import-errors-ko");
}

/// The real worker, counting the photos asked to be read ahead.
struct Counting {
    inner: grainy_app::worker::Worker,
    prefetched: Arc<AtomicUsize>,
}

impl grainy_app::render::Renderer for Counting {
    fn preview(&mut self, job: grainy_app::render::PreviewJob) {
        self.inner.preview(job);
    }
    fn thumbnail(&mut self, job: grainy_app::render::ThumbnailJob) {
        self.inner.thumbnail(job);
    }
    fn prefetch(&mut self, jobs: Vec<grainy_app::render::PrefetchJob>) {
        self.prefetched.fetch_add(jobs.len(), Ordering::SeqCst);
        self.inner.prefetch(jobs);
    }
    fn poll(&mut self) -> Vec<grainy_app::render::Reply> {
        self.inner.poll()
    }
    fn idle(&self) -> bool {
        self.inner.idle()
    }
    fn set_waker(&mut self, waker: grainy_app::render::Waker) {
        self.inner.set_waker(waker);
    }
}

/// Opening a photo refreshes its stored metadata (`catalog.update(photo_id, metadata=...)` after the load) and
/// reads the photos next to it ahead (`prefetch_neighbors`).
#[test]
fn opening_a_photo_refreshes_its_metadata_and_prefetches_neighbours() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("libcol-metadata");
    let shots = dir.join("Shots");
    std::fs::create_dir_all(&shots).unwrap();
    common::eight_photos(&shots);
    let library = dir.join("Library");
    let prefetched = Arc::new(AtomicUsize::new(0));
    let counter = prefetched.clone();
    let opened = library.clone();
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let doc = CatalogDocument::open(&opened).expect("library");
        let renderer = Counting { inner: grainy_app::worker::Worker::new(), prefetched: counter };
        let mut app = GrainyApp::with_document(&cc.egui_ctx, Box::new(renderer), Box::new(doc));
        app.open_paths(vec![shots.clone()]);
        app
    });
    settle(&mut harness);
    let first = harness.state().document().current_id();
    assert!(first > 0);
    assert_eq!(prefetched.load(Ordering::SeqCst), 1, "the first photo has one neighbour");

    // The stored metadata is emptied behind the window's back; opening the photo again writes the loader's.
    let writer = Catalog::new(&library).unwrap();
    let mut fields = serde_json::Map::new();
    fields.insert("metadata".into(), json!("{}"));
    writer.update(first as i64, &fields).unwrap();
    harness.state_mut().select_photo(1);
    settle(&mut harness);
    assert_eq!(prefetched.load(Ordering::SeqCst), 3, "the second photo has two neighbours");
    harness.state_mut().select_photo(0);
    settle(&mut harness);
    let info = writer.photo(first as i64).unwrap().unwrap()["info"].clone();
    assert_eq!(info["width"].as_u64(), Some(480), "{info}");
    assert!(info["file_fingerprint"].is_object() && info["working_space"].is_string(), "{info}");
}
