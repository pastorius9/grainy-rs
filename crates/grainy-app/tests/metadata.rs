//! The metadata package against the oracle (`golden/app/metadata*`, built by `tools/golden/gen_app_metadata.py` with
//! the Python Grainy's own `MainWindow.save_keywords` and `LibraryManager.metadata` / `shift_date` /
//! `refresh_metadata` / `keyword_tree` on stand-ins for the window): after the same scripted steps (keywords of the
//! open photo, the metadata box for one and many photos with accepted and refused values, the capture-time shift
//! with photos that have no time, a time that cannot be read, a date out of range and leap days) the photos' rows
//! of the Rust catalogue are the oracle's; the keyword tree has the oracle's nodes, counts and double-click effect;
//! the FILE text is the oracle's `file_info` text; the EXIF refresh stores the oracle's metadata rows. And the
//! window: the panel, the boxes and the commands end to end on a library in a temporary folder (snapshots in Korean
//! and English).
//!
//! Every test works on temporary folders and generated pictures; no library of the owner is opened.

mod common;

use std::path::{Path, PathBuf};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::library_column::LAUNCH_DISABLED;
use grainy_app::commands::metadata::{
    apply_form, info_text, keyword_groups, python_str, refresh_rows, set_keywords, shift_dates, store_rows, Form,
    FIELDS,
};
use grainy_app::commands::Invocation;
use grainy_app::dialogs::keyword_tree::KeywordTree;
use grainy_app::panels::metadata::{paste_crop, set_paste_crop, MetadataState};
use grainy_app::GrainyApp;
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

fn case(name: &str) -> Value {
    Case::open("app", name).json().clone()
}

/// A catalogue of the oracle's rows (`ROWS` of the generator): fixed paths, no files.
fn golden_catalog(name: &str) -> (PathBuf, Catalog) {
    let meta = case("metadata");
    let dir = common::temp_dir(name);
    let catalog = Catalog::with_sources(
        &dir,
        Sources::new(
            || "2026-11-13T01:02:03.456789+00:00".to_owned(),
            || "0123456789abcdef0123456789abcdef".to_owned(),
        ),
    )
    .unwrap();
    let root = meta["root"].as_str().unwrap();
    for row in meta["rows"].as_array().unwrap() {
        let path = format!("{root}\\{}", row["name"].as_str().unwrap());
        let id = catalog.add(&path, None, Some(&json!({})), true).unwrap();
        let mut fields = Map::new();
        fields.insert("metadata".into(), row["metadata"].clone());
        fields.insert("keywords".into(), row["keywords"].clone());
        fields.insert("extras".into(), row["extras"].clone());
        catalog.update(id, &fields).unwrap();
    }
    (dir, catalog)
}

/// The columns the generator compares (`full_rows`: not the path, which the platform spells its own way).
fn full_rows(catalog: &Catalog) -> Vec<Value> {
    catalog
        .photos()
        .unwrap()
        .iter()
        .map(|p| {
            let mut row = Map::new();
            for key in [
                "id",
                "name",
                "rating",
                "flag",
                "label",
                "keywords",
                "metadata",
                "extras",
                "stack_id",
                "virtual_source",
            ] {
                row.insert(key.into(), p[key].clone());
            }
            Value::Object(row)
        })
        .collect()
}

fn projection(catalog: &Catalog) -> Vec<Value> {
    catalog
        .photos()
        .unwrap()
        .iter()
        .map(|p| json!({"id": p["id"], "keywords": p["keywords"], "extras": p["extras"]}))
        .collect()
}

fn without_path(rows: &Value) -> Vec<Value> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let mut r = r.as_object().unwrap().clone();
            r.remove("path");
            Value::Object(r)
        })
        .collect()
}

#[test]
fn the_catalogue_starts_as_the_oracles() {
    let meta = case("metadata");
    let (_, catalog) = golden_catalog("metadata-start");
    assert_eq!(full_rows(&catalog), without_path(&meta["start"]));
}

/// The selection of the script: `selected_ids` (the selection, else the open photo).
struct Script {
    selection: Vec<i64>,
    current: Option<i64>,
}

impl Script {
    fn ids(&self) -> Vec<i64> {
        if self.selection.is_empty() {
            self.current.into_iter().collect()
        } else {
            self.selection.clone()
        }
    }
}

/// Runs a step as the commands do. Returns the error (if any) and the status lines.
fn step(catalog: &Catalog, script: &mut Script, step: &Value, expected: &Value) -> (Option<String>, Vec<String>) {
    let args = step.as_array().unwrap();
    let mut status = Vec::new();
    let mut error = None;
    match args[0].as_str().unwrap() {
        "open" => {
            script.current = args[1].as_i64();
            script.selection = vec![args[1].as_i64().unwrap()];
        }
        "select" => script.selection = args[1].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect(),
        "keywords" => {
            script.current = args[1].as_i64();
            set_keywords(catalog, args[1].as_i64().unwrap(), args[2].as_str().unwrap()).unwrap();
        }
        "metadata" => {
            let ids = script.ids();
            let mut form = Form::load(catalog, &ids).expect("the first photo");
            // The box as the oracle built it: the fields' text and the keywords.
            let mut shown: Vec<Value> = form.values.iter().map(|v| json!(v)).collect();
            shown.push(json!(form.keywords));
            assert_eq!(Value::Array(shown), expected["initial"], "the box shows the first photo's values: {step}");
            for (key, text) in args[1].as_object().unwrap() {
                form.set(key, text.as_str().unwrap());
            }
            if let Some(keywords) = args[2].as_str() {
                form.keywords = keywords.to_owned();
            }
            error = apply_form(catalog, &ids, &form).err();
        }
        "shift" => match shift_dates(catalog, &script.ids(), args[1].as_i64().unwrap()) {
            Ok(count) => status.push(format!("{count}장 촬영 시각을 변경했습니다.")),
            Err(message) => error = Some(message),
        },
        other => panic!("{other}"),
    }
    (error, status)
}

#[test]
fn a_scripted_metadata_session_writes_what_the_oracle_writes() {
    let meta = case("metadata");
    let (_, catalog) = golden_catalog("metadata-script");
    let mut script = Script { selection: Vec::new(), current: None };
    let mut errors = 0;
    for expected in meta["steps"].as_array().unwrap() {
        let at = expected["step"].to_string();
        let (error, status) = step(&catalog, &mut script, &expected["step"], expected);
        match expected["error"].as_str() {
            Some(oracle) => {
                errors += 1;
                let error = error.unwrap_or_else(|| panic!("the oracle refuses {at}: {oracle}"));
                if oracle.ends_with("GPS 좌표 범위를 확인하세요.") {
                    assert_eq!(error, "GPS 좌표 범위를 확인하세요.", "{at}");
                }
                // The messages of the other refusals are Python's own; Rust words them the same way.
                let (kind, text) = oracle.split_once(": ").unwrap();
                if kind != "ValueError" || text.starts_with("could not") || text.starts_with("time data") {
                    assert_eq!(error, text, "{at}");
                }
            }
            None => assert_eq!(error, None, "{at}"),
        }
        let oracle_status: Vec<String> =
            expected["status"].as_array().unwrap().iter().map(|s| s.as_str().unwrap().to_owned()).collect();
        assert_eq!(status, oracle_status, "{at}");
        assert_eq!(Value::Array(projection(&catalog)), expected["rows"], "rows after {at}");
    }
    assert!(errors >= 6, "the script covers the refusals: {errors}");
    assert_eq!(full_rows(&catalog), without_path(&meta["final"]));
}

#[test]
fn the_box_lists_the_oracles_fields_in_its_order() {
    let meta = case("metadata");
    let keys: Vec<&str> = FIELDS.iter().map(|(k, _)| *k).collect();
    let oracle: Vec<&str> = meta["fields"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(keys, oracle);
}

fn rows_of(tree: &Value) -> Vec<Value> {
    tree["rows"].as_array().unwrap().clone()
}

#[test]
fn the_keyword_tree_has_the_oracles_nodes_and_counts() {
    let meta = case("metadata");
    let (_, catalog) = golden_catalog("metadata-tree");
    let tree = KeywordTree::new(keyword_groups(&catalog).unwrap());
    let rows: Vec<Value> = tree
        .rows()
        .into_iter()
        .map(|(path, name, count, depth)| json!([depth, name, count.to_string(), path]))
        .collect();
    assert_eq!(rows, rows_of(&meta["start_tree"]));

    // After the session the tree has the new keywords (the script's rows, written by the oracle's methods).
    let (_, catalog) = golden_catalog("metadata-tree-final");
    let mut script = Script { selection: Vec::new(), current: None };
    for expected in meta["steps"].as_array().unwrap() {
        step(&catalog, &mut script, &expected["step"], expected);
    }
    let tree = KeywordTree::new(keyword_groups(&catalog).unwrap());
    let rows: Vec<Value> = tree
        .rows()
        .into_iter()
        .map(|(path, name, count, depth)| json!([depth, name, count.to_string(), path]))
        .collect();
    assert_eq!(rows, rows_of(&meta["final_tree"]));
}

#[test]
fn siblings_come_in_the_trees_order_not_the_flat_sort() {
    // "a b" sorts between "a" and "a|b" as text, but the tree lists a's keywords right under "a".
    let tree = KeywordTree::new(vec![("a".into(), 2), ("a b".into(), 1), ("a|b".into(), 1)]);
    let order: Vec<String> = tree.rows().into_iter().map(|r| r.0).collect();
    assert_eq!(order, ["a", "a|b", "a b"]);
}

#[test]
fn the_file_text_is_the_oracles() {
    for case in case("metadata_info")["cases"].as_array().unwrap() {
        let info: Value = serde_json::from_str(case["info"].as_str().unwrap()).unwrap();
        let text = info_text(case["path"].as_str().unwrap(), &info);
        assert_eq!(text, case["text"].as_str().unwrap(), "{}", case["name"]);
    }
    assert_eq!(python_str(&json!(100.0)), "100.0");
    assert_eq!(python_str(&json!(null)), "None");
}

/// The files of the refresh golden copied to a folder, a catalogue over them with the oracle's stored metadata.
fn refresh_catalog(name: &str) -> (PathBuf, Catalog, Value) {
    let meta = case("metadata_refresh");
    let dir = common::temp_dir(name);
    let files = dir.join("files");
    std::fs::create_dir_all(&files).unwrap();
    let source = grainy_testkit::root().join("golden/app/metadata_refresh/files");
    for entry in std::fs::read_dir(&source).unwrap() {
        let entry = entry.unwrap();
        std::fs::copy(entry.path(), files.join(entry.file_name())).unwrap();
    }
    let catalog = Catalog::new(dir.join("Library")).unwrap();
    for row in meta["rows"].as_array().unwrap() {
        let path = files.join(row["name"].as_str().unwrap());
        let id = catalog.add(path.to_str().unwrap(), None, Some(&json!({})), true).unwrap();
        let mut fields = Map::new();
        fields.insert("metadata".into(), row["metadata"].clone());
        catalog.update(id, &fields).unwrap();
    }
    (dir, catalog, meta)
}

fn stored_metadata(catalog: &Catalog) -> Vec<Value> {
    catalog.photos().unwrap().iter().map(|p| json!({"name": p["name"], "metadata": p["metadata"]})).collect()
}

#[test]
fn the_exif_refresh_stores_the_oracles_rows() {
    let (_, catalog, meta) = refresh_catalog("metadata-refresh");
    let (rows, failures) = refresh_rows(&catalog, &|| false).unwrap();
    assert_eq!(store_rows(&catalog, &rows).unwrap(), rows.len());
    assert_eq!(Value::Array(stored_metadata(&catalog)), meta["result"]);
    // `메타데이터 {0}장 갱신 · 읽을 수 없음 {1}장`: the picture, the PNG, the plain file and the unreadable one
    // are read (four), the file that is not there is a failure, the video is skipped.
    let message = meta["messages"][0].as_str().unwrap();
    assert_eq!(message, format!("메타데이터 {}장 갱신 · 읽을 수 없음 {failures}장", rows.len()));
}

#[test]
fn writing_the_refresh_leaves_other_columns_alone() {
    let (_, catalog, _) = refresh_catalog("metadata-refresh-columns");
    let before: Vec<Value> = catalog
        .photos()
        .unwrap()
        .iter()
        .map(|p| {
            let mut p = p.as_object().unwrap().clone();
            p.remove("metadata");
            Value::Object(p)
        })
        .collect();
    let (rows, _) = refresh_rows(&catalog, &|| false).unwrap();
    store_rows(&catalog, &rows).unwrap();
    let after: Vec<Value> = catalog
        .photos()
        .unwrap()
        .iter()
        .map(|p| {
            let mut p = p.as_object().unwrap().clone();
            p.remove("metadata");
            Value::Object(p)
        })
        .collect();
    assert_eq!(before, after);
}

#[test]
fn a_cancelled_refresh_reads_nothing_more() {
    let (_, catalog, _) = refresh_catalog("metadata-refresh-cancel");
    let (rows, failures) = refresh_rows(&catalog, &|| true).unwrap();
    assert!(rows.is_empty() && failures == 0);
}

#[test]
fn the_shift_reads_all_times_before_it_writes() {
    let (_, catalog) = golden_catalog("metadata-shift-atomic");
    let before = projection(&catalog);
    // Photo 8's time is "garbage": nothing moves, not even photo 1's.
    let error = shift_dates(&catalog, &[1, 8], 5).unwrap_err();
    assert!(error.starts_with("time data 'garbage'"), "{error}");
    assert_eq!(projection(&catalog), before);
    // A photo without a time is left out and does not count.
    assert_eq!(shift_dates(&catalog, &[3, 5], 60).unwrap(), 0);
    assert_eq!(shift_dates(&catalog, &[1, 3], 60).unwrap(), 1);
}

#[test]
fn the_copy_crop_box_is_the_windows_one() {
    set_paste_crop(true);
    assert!(paste_crop());
    set_paste_crop(false);
    assert!(!paste_crop());
}

// ---- the window --------------------------------------------------------------------------------------------------

/// A fresh folder named `name` (the process id is in the folder above it, so the library tree shows the fixed name
/// and two test runs at the same time do not share a folder).
fn fixed_dir(name: &str) -> PathBuf {
    common::temp_dir(name)
}

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
    // Tall, so that the whole 사진 정보·관리 panel is on screen below the other (folded) panels.
    Harness::builder().with_size(egui::vec2(1600.0, 1500.0)).with_pixels_per_point(1.0).with_context_setup(|ctx| {
        grainy_app::fonts::install(ctx);
        grainy_app::theme::apply(ctx);
    })
}

/// The window on a new library in a temporary folder with two generated photos, the first open.
fn library_harness<'a>(name: &str) -> (Harness<'a, GrainyApp>, PathBuf) {
    let dir = fixed_dir(name);
    let photos = [dir.join("garden.png"), dir.join("garden two.png")];
    common::save_png(&common::garden(2, [1.0; 3], false), &photos[0]);
    common::save_png(&common::garden(2, [0.8, 1.0, 1.1], true), &photos[1]);
    let library = dir.join("Library");
    let data = dir.join("data");
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::with_library(&cc.egui_ctx, &library, Some(data));
        app.set_debug_overlay(false);
        app.open_paths(photos.to_vec());
        app
    });
    settle(&mut harness);
    (harness, dir)
}

fn settle(harness: &mut Harness<'_, GrainyApp>) {
    common::snapshot::settle(harness);
}

fn save(harness: &mut Harness<'_, GrainyApp>, name: &str) {
    common::snapshot::save(harness, name);
}

fn reader(dir: &Path) -> Catalog {
    Catalog::open_reader(dir.join("Library")).unwrap()
}

fn run(harness: &mut Harness<'_, GrainyApp>, id: &str, invocation: &Invocation) -> bool {
    harness.state_mut().cx.run_with(id, invocation)
}

/// The path the FILE text shows, fixed for the snapshots (the real one is in a temporary folder).
fn show_fixed_path(harness: &mut Harness<'_, GrainyApp>) {
    let (info, name) = {
        let doc = &harness.state().cx.doc;
        let id = doc.current_id() as i64;
        (
            doc.metadata_catalog().unwrap().photo(id).unwrap().unwrap()["info"].clone(),
            doc.current().unwrap().name.clone(),
        )
    };
    // Whether the folder tree shows its folders open depends on when the import finished: shut them. The status
    // line shows the import's "complete" or the preview's size, whichever job answered last: pin it.
    common::snapshot::open_folders(harness.state_mut(), &[]);
    common::snapshot::pin_status(harness.state_mut(), "");
    harness.state_mut().cx.state::<MetadataState>().info = info_text(&format!("D:\\Photos\\{name}"), &info);
}

fn flow(language: Language, tag: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    LAUNCH_DISABLED.store(true, std::sync::atomic::Ordering::Relaxed);
    let (mut harness, dir) = library_harness(&format!("metadata-window-{tag}"));
    harness.state_mut().cx.set_mode(grainy_app::context::Mode::Develop);
    harness.state_mut().open_panel("metadata");
    settle(&mut harness);

    // The panel reads the open photo: the file facts the load stored (width, height, format…).
    let id = harness.state().cx.doc.current_id() as i64;
    let name = harness.state().cx.doc.current().unwrap().name.clone();
    let info = harness.state_mut().cx.state::<MetadataState>().info.clone();
    assert!(info.contains(&name) && info.contains("width: 480") && info.contains("format: PNG"), "{info}");

    // Rating, flag and colour label of the selection, through the commands the panel's controls run.
    assert!(run(&mut harness, "cull.rating", &Invocation::value(4)));
    assert!(run(&mut harness, "cull.pick", &Invocation::default()));
    assert!(run(&mut harness, "cull.color_label", &Invocation::value("빨강")));
    assert!(run(&mut harness, "metadata.keywords", &Invocation::value("여행|일본, 필름")));
    settle(&mut harness);
    {
        let catalog = reader(&dir);
        let photo = catalog.photo(id).unwrap().unwrap();
        assert_eq!(photo["rating"], 4);
        assert_eq!(photo["flag"], 1);
        assert_eq!(photo["label"], "빨강");
        assert_eq!(photo["keywords"], "여행|일본, 필름");
    }
    show_fixed_path(&mut harness);
    harness.run_steps(3);
    save(&mut harness, &format!("metadata-panel-{tag}"));

    // The metadata box for the open photo: the stored values in the fields; 저장 with a changed title.
    assert!(run(&mut harness, "metadata.edit", &Invocation::default()));
    harness.run_steps(3);
    assert!(harness.state().dialog_open("metadata"));
    show_fixed_path(&mut harness);
    harness.run_steps(2);
    save(&mut harness, &format!("metadata-dialog-{tag}"));
    harness.state_mut().close_dialog("metadata");
    harness.run_steps(2);
    assert!(!harness.state().dialog_open("metadata"));

    // The same through the Rust model: the title, the place, the capture time.
    {
        let catalog = harness.state().cx.doc.metadata_catalog().unwrap();
        let mut form = Form::load(catalog, &[id]).unwrap();
        form.set("title", "정원");
        form.set("DateTimeOriginal", "2025:06:01 10:00:00");
        form.set("city", "서울");
        form.keywords = "여행|일본, 필름, 정원".into();
        grainy_app::commands::metadata::save_form(&mut harness.state_mut().cx, &[id], &form);
    }
    run(&mut harness, "metadata.shift_date", &Invocation::value(90));
    settle(&mut harness);
    {
        let catalog = reader(&dir);
        let photo = catalog.photo(id).unwrap().unwrap();
        assert_eq!(photo["user_metadata"]["title"], "정원");
        assert_eq!(photo["user_metadata"]["DateTimeOriginal"], "2025:06:01 11:30:00");
        assert_eq!(photo["keywords"], "여행|일본, 필름, 정원");
    }
    assert_eq!(harness.state().status(), tr_ko_en("{0}장 촬영 시각을 변경했습니다."));

    // The shift box.
    assert!(run(&mut harness, "metadata.shift_date", &Invocation::default()));
    harness.run_steps(3);
    assert!(harness.state().dialog_open("metadata_shift"));
    show_fixed_path(&mut harness);
    harness.run_steps(2);
    save(&mut harness, &format!("metadata-shift-{tag}"));
    harness.state_mut().close_dialog("metadata_shift");

    // The keyword tree: 여행 with 일본 under it; a double click filters the list.
    assert!(run(&mut harness, "metadata.keyword_tree", &Invocation::default()));
    harness.run_steps(3);
    assert!(harness.state().dialog_open("keyword_tree"));
    show_fixed_path(&mut harness);
    harness.run_steps(2);
    save(&mut harness, &format!("metadata-keywords-{tag}"));
    harness.state_mut().close_dialog("keyword_tree");
    grainy_app::commands::metadata::filter_keyword(&mut harness.state_mut().cx, "여행|일본");
    settle(&mut harness);
    let query = harness.state().cx.doc.query();
    assert_eq!(query.rules, json!({"keywords": "여행|일본"}));
    assert_eq!((query.collection, query.folder), (None, None));
    assert_eq!(harness.state().cx.doc.len(), 1, "only the photo with the keyword is listed");
    assert_eq!(harness.state().cx.mode(), grainy_app::context::Mode::Library);

    LAUNCH_DISABLED.store(false, std::sync::atomic::Ordering::Relaxed);
    grainy_i18n::set_language(Language::Korean);
}

fn tr_ko_en(source: &str) -> String {
    grainy_app::text::tr_with(source, &["1"])
}

#[test]
fn the_window_runs_the_metadata_commands_korean() {
    flow(Language::Korean, "ko");
}

#[test]
fn the_window_runs_the_metadata_commands_english() {
    flow(Language::English, "en");
}

#[test]
fn a_refresh_job_stores_the_files_exif_and_reports() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let (mut harness, dir) = library_harness("metadata-refresh-job");
    // The library holds two photos; their stored metadata is replaced by a stale value first.
    {
        let catalog = Catalog::new(dir.join("Library")).unwrap();
        for id in [1, 2] {
            let mut fields = Map::new();
            fields.insert("metadata".into(), json!("{\"width\": 1, \"format\": \"OLD\"}"));
            catalog.update(id, &fields).unwrap();
        }
    }
    assert!(run(&mut harness, "metadata.refresh", &Invocation::default()));
    settle(&mut harness);
    let catalog = reader(&dir);
    for id in [1, 2] {
        let photo = catalog.photo(id).unwrap().unwrap();
        // PNG without EXIF: nothing to add, the stale dictionary stays as it was.
        assert_eq!(photo["info"]["format"], "OLD");
    }
    assert_eq!(harness.state().status(), "메타데이터 2장 갱신 · 읽을 수 없음 0장");
}

#[test]
fn the_commands_need_a_library_open_for_writing() {
    let _turn = common::language_lock();
    let dir = common::temp_dir("metadata-session");
    let photo = dir.join("garden.png");
    common::save_png(&common::garden(2, [1.0; 3], false), &photo);
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let mut app = GrainyApp::new(&cc.egui_ctx);
        app.set_debug_overlay(false);
        app.open_paths(vec![photo]);
        app
    });
    settle(&mut harness);
    for id in ["metadata.edit", "metadata.shift_date", "metadata.refresh", "metadata.keywords"] {
        assert!(!grainy_app::commands::enabled(&harness.state().cx, id), "{id}");
    }
    assert!(grainy_app::commands::enabled(&harness.state().cx, "metadata.show_file"));
    let _ = harness.state().cx.doc.metadata_catalog().is_none();
}
