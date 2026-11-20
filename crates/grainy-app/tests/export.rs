//! Work package 13: export completion, against the oracle (`golden/app/export`, built by
//! `tools/golden/gen_app_export.py` with the Python Grainy's own `ExportDialog`, `quick_export`, `start_export`,
//! `show_exports` and `reset_exports` run on a stand-in for the window over a real library of generated photos).
//!
//! The model tests drive the context without a window (jobs are waited for, native folder dialogs are not opened:
//! the quick export's plan and its folder answer are separate calls); the window tests render the real
//! application headless and write `tests/snapshots/export-*`. Every test works on generated photos in temporary
//! folders.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::commands::export::{self as commands, Quick};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::export::{self, ExportOptions, COLOR_SPACES, FORMATS};
use grainy_app::render::TestCard;
use grainy_app::{AppContext, GrainyApp, Mode};
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

fn korean() -> std::sync::MutexGuard<'static, ()> {
    let turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    turn
}

fn golden() -> Value {
    Case::open("app", "export").json().clone()
}

const PREFERENCE: &str = "previous_photo_export";
const HISTORY: &str = "export_history";

// ---- the library --------------------------------------------------------------------------------------------

/// The generator's library: four photos (48 × 32) with edits and a fifth whose file is gone.
struct Library {
    src: PathBuf,
    out: PathBuf,
    missing: PathBuf,
    cx: AppContext,
}

fn photo(path: &Path, gain: [f32; 3]) {
    let image = image::RgbImage::from_fn(48, 32, |x, y| {
        let (fx, fy) = (x as f32 / 47.0, y as f32 / 31.0);
        let v = [fx, fy, (1.0 - fx) * (1.0 - fy)];
        image::Rgb([0, 1, 2].map(|c| ((v[c] * gain[c]).clamp(0.0, 1.0) * 255.0 + 0.5) as u8))
    });
    image.save(path).expect("write the photo");
}

fn library(name: &str) -> Library {
    let meta = golden();
    let stamp = meta["stamp"].as_str().unwrap().to_owned();
    let dir = common::temp_dir(name);
    let (src, out, lib) = (dir.join("src"), dir.join("out"), dir.join("lib"));
    for folder in [&src, &out, &lib] {
        std::fs::create_dir_all(folder).unwrap();
    }
    let catalog = Catalog::with_sources(&lib, Sources::new(move || stamp.clone(), || "0".repeat(32))).unwrap();
    let info = json!({"format": "png", "width": 48, "height": 32});
    let photos: [(&str, [f32; 3], Value); 4] = [
        ("a.png", [1.0, 1.0, 1.0], json!({"exposure": 0.4})),
        ("b.png", [1.0, 0.8, 0.6], json!({"saturation": 12.0, "rotation": 1})),
        ("c.png", [0.9, 0.9, 0.9], json!({"monochrome": true})),
        ("d.png", [0.6, 0.8, 1.0], json!({})),
    ];
    for (name, gain, settings) in photos {
        photo(&src.join(name), gain);
        catalog.add(&src.join(name).to_string_lossy(), Some(&common::settings(settings)), Some(&info), false).unwrap();
    }
    photo(&src.join("gone.png"), [1.0; 3]);
    let gone = common::settings(json!({"contrast": 10.0}));
    catalog.add(&src.join("gone.png").to_string_lossy(), Some(&gone), Some(&info), false).unwrap();
    std::fs::remove_file(src.join("gone.png")).unwrap();
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc));
    let first = cx.doc.index_of(1).expect("photo 1 is listed");
    cx.select_photo(first);
    Library { missing: dir.join("missing"), src, out, cx }
}

fn select_ids(cx: &mut AppContext, ids: &[u64]) {
    let first = cx.doc.index_of(ids[0]).unwrap_or_else(|| panic!("photo {} is listed", ids[0]));
    cx.select_photo(first);
    for id in &ids[1..] {
        let index = cx.doc.index_of(*id).unwrap();
        cx.doc.toggle_selected(index);
    }
}

/// Runs the finished jobs of the context until none of `kind` is left.
fn wait(cx: &mut AppContext, kind: &str) {
    let started = Instant::now();
    while cx.jobs.running(kind) {
        for finished in cx.jobs.poll() {
            finished(cx);
        }
        assert!(started.elapsed() < Duration::from_secs(120), "the {kind} job did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A path as the generator stores it: the temporary folders replaced and `/` for `\`.
fn rel(text: &str, roots: &[(&Path, &str)]) -> String {
    let mut text = text.to_owned();
    for (folder, marker) in roots {
        text = text.replace(folder.to_string_lossy().as_ref(), marker);
    }
    text.replace('\\', "/")
}

fn rel_value(value: &Value, roots: &[(&Path, &str)]) -> Value {
    match value {
        Value::String(s) => Value::String(rel(s, roots)),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), rel_value(v, roots))).collect()),
        Value::Array(list) => Value::Array(list.iter().map(|v| rel_value(v, roots)).collect()),
        other => other.clone(),
    }
}

/// The reverse of [`rel`] for the placeholders `<out>` and `<missing>`.
fn real(value: &Value, library: &Library) -> Value {
    let (out, missing) = (library.out.to_string_lossy(), library.missing.to_string_lossy());
    match value {
        Value::String(s) => Value::String(s.replace("<out>", &out).replace("<missing>", &missing)),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), real(v, library))).collect()),
        other => other.clone(),
    }
}

fn options_with(map: &Value, folder: &Path) -> ExportOptions {
    let mut map = map.as_object().cloned().unwrap();
    map.insert("folder".into(), folder.to_string_lossy().into_owned().into());
    ExportOptions::from_saved(&map)
}

// ---- the dialog ---------------------------------------------------------------------------------------------

#[test]
fn the_key_order_of_the_sidecar_is_the_oracles() {
    let case = golden();
    let oracle: Vec<&str> = case["defaults_order"].as_array().unwrap().iter().map(|k| k.as_str().unwrap()).collect();
    assert_eq!(export::DEFAULTS_ORDER, oracle.as_slice());
    let ours = grainy_core::defaults();
    let mut keys: Vec<&String> = ours.0.keys().collect();
    keys.sort();
    let mut listed: Vec<&str> = export::DEFAULTS_ORDER.to_vec();
    listed.sort_unstable();
    assert_eq!(keys.iter().map(|k| k.as_str()).collect::<Vec<_>>(), listed, "the same keys as the Rust defaults");
    let ordered = export::python_order(&ours.0);
    assert_eq!(ordered.keys().map(String::as_str).collect::<Vec<_>>(), oracle);
}

#[test]
fn the_format_list_and_the_fields_each_format_enables() {
    let _turn = korean();
    let case = golden();
    let formats = &case["dialog_formats"];
    let listed: Vec<(String, String)> = formats["listed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p[0].as_str().unwrap().to_owned(), p[1].as_str().unwrap().to_owned()))
        .collect();
    let ours: Vec<(String, String)> = export::listed_formats()
        .iter()
        .map(|i| FORMATS[*i])
        .map(|f| (f.to_owned(), grainy_app::text::tr(export::format_label(f))))
        .collect();
    assert_eq!(ours, listed, "the format list (the HDR TIFF is hidden while HDR_FEATURE is off)");
    for row in formats["rows"].as_array().unwrap() {
        let name = row["format"].as_str().unwrap();
        let mut options = ExportOptions { color_space: 1, ..ExportOptions::default() };
        options.set_format(FORMATS.iter().position(|f| *f == name).unwrap());
        let fields = options.fields();
        let enabled = &row["enabled"];
        for (key, value) in [
            ("quality", fields.quality),
            ("color_space", fields.color_space),
            ("longest", fields.longest),
            ("keep_metadata", fields.keep_metadata),
            ("grayscale", fields.grayscale),
            ("dither", fields.dither),
        ] {
            assert_eq!(enabled[key].as_bool().unwrap(), value, "{name}: {key}");
        }
        assert_eq!(COLOR_SPACES[options.color_space], row["color_space"].as_str().unwrap(), "{name}: colour space");
    }
    let initial = &formats["initial"];
    let defaults = ExportOptions::default();
    assert_eq!(defaults.format_name(), initial["format"].as_str().unwrap());
    assert_eq!(i64::from(defaults.quality), initial["quality"].as_i64().unwrap());
    assert_eq!(defaults.naming, initial["naming"].as_str().unwrap());
    assert_eq!(defaults.grayscale, initial["grayscale"].as_bool().unwrap());
    assert_eq!(defaults.dither, initial["dither"].as_bool().unwrap());
    assert_eq!(40, formats["minimum_quality"].as_i64().unwrap());
    assert_eq!(30000, formats["maximum_longest"].as_i64().unwrap());
}

#[test]
fn validate_gives_the_oracles_messages() {
    let _turn = korean();
    let l = library("export-validate");
    let cases = golden()["dialog_validate"].as_array().unwrap().clone();
    assert!(cases.len() >= 29);
    for case in cases {
        let folder = match case["folder"].as_str().unwrap() {
            "empty" => String::new(),
            "blank" => "   ".to_owned(),
            "missing" => l.missing.to_string_lossy().into_owned(),
            _ => l.out.to_string_lossy().into_owned(),
        };
        let options =
            ExportOptions { folder, naming: case["naming"].as_str().unwrap().to_owned(), ..Default::default() };
        let result = options.validate();
        let naming = &case["naming"];
        if case["accepted"].as_bool().unwrap() {
            assert_eq!(result, Ok(()), "{naming}");
        } else {
            let text = case["box"][2].as_str().unwrap();
            assert_eq!(result, Err(text), "{naming}: {}", case["folder"]);
        }
    }
}

#[test]
fn the_dialog_opens_with_the_previous_export() {
    let _turn = korean();
    let mut l = library("export-previous");
    let cases = golden()["dialog_previous"].as_object().unwrap().clone();
    assert!(cases.len() >= 11);
    for (name, case) in cases {
        let saved = case["saved"].clone();
        l.cx.doc.save_preference(PREFERENCE, &saved).unwrap();
        let options = commands::dialog_options(&l.cx);
        let state = &case["state"];
        assert_eq!(options.folder, state["folder"].as_str().unwrap(), "{name}: folder");
        assert_eq!(options.format_name(), state["format"].as_str().unwrap(), "{name}: format");
        assert_eq!(i64::from(options.quality), state["quality"].as_i64().unwrap(), "{name}: quality");
        assert_eq!(i64::from(options.longest), state["longest"].as_i64().unwrap(), "{name}: longest");
        assert_eq!(COLOR_SPACES[options.color_space], state["color_space"].as_str().unwrap(), "{name}: space");
        assert_eq!(options.keep_metadata, state["keep_metadata"].as_bool().unwrap(), "{name}: metadata");
        assert_eq!(options.grayscale, state["grayscale"].as_bool().unwrap(), "{name}: grayscale");
        assert_eq!(options.dither, state["dither"].as_bool().unwrap(), "{name}: dither");
        assert_eq!(options.naming, state["naming"].as_str().unwrap(), "{name}: naming");
        let fields = options.fields();
        let enabled = &case["enabled"];
        assert_eq!(fields.quality, enabled["quality"].as_bool().unwrap(), "{name}");
        assert_eq!(fields.color_space, enabled["color_space"].as_bool().unwrap(), "{name}");
        assert_eq!(fields.longest, enabled["longest"].as_bool().unwrap(), "{name}");
        assert_eq!(fields.keep_metadata, enabled["keep_metadata"].as_bool().unwrap(), "{name}");
        assert_eq!(fields.grayscale, enabled["grayscale"].as_bool().unwrap(), "{name}");
        assert_eq!(fields.dither, enabled["dither"].as_bool().unwrap(), "{name}");
    }
}

#[test]
fn saved_values_round_trip_through_the_preference() {
    let _turn = korean();
    let l = library("export-saved");
    for (name, saved) in golden()["dialog_previous"].as_object().unwrap() {
        let Some(map) = export::previous(Some(&saved["saved"])) else { continue };
        let options = ExportOptions::from_saved(&map);
        let again = options.saved();
        for key in ["folder", "format", "quality", "longest", "keep_metadata", "naming", "grayscale", "dither"] {
            assert_eq!(again[key], map[key], "{name}: {key}");
        }
        assert_eq!(again["color_space"], map["color_space"], "{name}");
    }
    drop(l);
}

// ---- the quick exports --------------------------------------------------------------------------------------

/// What the plan did, in the generator's terms: the exports started `(folder, options)`, whether the export
/// dialog opened and the folder dialogs asked `(title, start)`.
type Done = (Vec<(String, Map<String, Value>)>, bool, Vec<(String, String)>);

fn interpret(plan: Quick, answer: &str) -> Done {
    let options_of = |options: &ExportOptions| {
        let mut map = options.saved();
        map.remove("folder");
        map
    };
    match plan {
        Quick::Nothing => (Vec::new(), false, Vec::new()),
        Quick::Dialog => (Vec::new(), true, Vec::new()),
        Quick::Start(options) => (vec![(options.folder.clone(), options_of(&options))], false, Vec::new()),
        Quick::Ask { title, start, options } => {
            let asked = vec![(grainy_app::text::tr(title), start)];
            let started = if answer.is_empty() { Vec::new() } else { vec![(answer.to_owned(), options_of(&options))] };
            (started, false, asked)
        }
    }
}

#[test]
fn the_quick_exports_decide_like_the_oracle() {
    let _turn = korean();
    let mut l = library("export-quick");
    let cases = golden()["quick"].as_array().unwrap().clone();
    assert!(cases.len() >= 20);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let saved = if case["saved"].is_null() { Value::Null } else { real(&case["saved"], &l) };
        l.cx.doc.save_preference(PREFERENCE, &saved).unwrap();
        let ids: Vec<u64> = case["selection"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
        let answer = case["answer"].as_str().unwrap().replace("<out>", &l.out.to_string_lossy());
        let mut empty = None;
        let cx = if ids.is_empty() {
            // Nothing selected: an empty library.
            let dir = common::temp_dir("export-quick-empty");
            let doc = CatalogDocument::open(&dir).unwrap();
            empty = Some(AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc)));
            empty.as_mut().unwrap()
        } else {
            select_ids(&mut l.cx, &ids);
            &mut l.cx
        };
        if case["running"].as_bool().unwrap() {
            // A running export: a job that waits until released.
            let (release, hold) = std::sync::mpsc::channel::<()>();
            cx.jobs.spawn(
                "export",
                "waiting",
                move |_| {
                    let _ = hold.recv_timeout(Duration::from_secs(60));
                    Ok(())
                },
                |_: &mut AppContext, _: Result<(), String>| {},
            );
            let plan = commands::plan_quick(cx, case["kind"].as_str().unwrap());
            assert_eq!(plan, Quick::Nothing, "{name}");
            assert!(!grainy_app::commands::enabled(cx, "file.quick_export"), "{name}: the command waits");
            let _ = release.send(());
            wait(cx, "export");
            continue;
        }
        let plan = commands::plan_quick(cx, case["kind"].as_str().unwrap());
        let (started, dialog, asked) = interpret(plan, &answer);
        let golden_started: Vec<(String, Map<String, Value>)> = case["started"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (rel(s["folder"].as_str().unwrap(), &[]), s["options"].as_object().unwrap().clone()))
            .collect();
        let roots: [(&Path, &str); 1] = [(&l.out, "<out>")];
        let started: Vec<(String, Map<String, Value>)> =
            started.into_iter().map(|(f, o)| (rel(&f, &roots), o)).collect();
        assert_eq!(started, golden_started, "{name}: what starts");
        assert_eq!(dialog, case["dialog_opened"].as_i64().unwrap() == 1, "{name}: the dialog");
        let golden_asked: Vec<(String, String)> = case["folder_dialogs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| (d[0].as_str().unwrap().to_owned(), d[1].as_str().unwrap().to_owned()))
            .collect();
        let missing = l.missing.clone();
        let asked: Vec<(String, String)> = asked
            .into_iter()
            .map(|(title, start)| (title, rel(&start, &[(&l.out, "<out>"), (&missing, "<missing>")])))
            .collect();
        assert_eq!(asked, golden_asked, "{name}: the folder dialog");
        drop(empty);
    }
}

#[test]
fn the_quick_export_in_the_picked_folder_writes_the_files_and_remembers_them() {
    let _turn = korean();
    let mut l = library("export-quick-run");
    select_ids(&mut l.cx, &[1, 2]);
    let Quick::Ask { options, .. } = commands::plan_quick(&mut l.cx, "small") else { panic!("a folder is asked for") };
    // The answer of the folder dialog starts the export.
    commands::finish_quick(&mut l.cx, options, Some(l.out.clone()));
    assert!(l.cx.jobs.running("export"));
    wait(&mut l.cx, "export");
    let mut names: Vec<String> =
        std::fs::read_dir(&l.out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["a.jpg", "b.jpg"], "naming {{stem}} of the preset");
    let saved = l.cx.doc.preference(PREFERENCE).expect("the previous export is saved");
    assert_eq!(saved["format"], "JPEG");
    assert_eq!(saved["quality"], 80);
    assert_eq!(saved["longest"], 2048);
    assert_eq!(saved["naming"], "{stem}");
    // "이전 설정으로 내보내기" now starts at once (the folder exists) and writes new names.
    let plan = commands::plan_quick(&mut l.cx, "previous");
    let Quick::Start(options) = plan else { panic!("the previous export starts at once: {plan:?}") };
    commands::start_export(&mut l.cx, options);
    wait(&mut l.cx, "export");
    assert!(l.out.join("a_2.jpg").is_file() && l.out.join("b_2.jpg").is_file());
}

// ---- the runs -----------------------------------------------------------------------------------------------

fn status_of(cx: &AppContext) -> String {
    cx.status().to_owned()
}

#[test]
fn exports_write_the_oracles_files_records_and_preferences() {
    let _turn = korean();
    let mut l = library("export-runs");
    let case = golden();
    let runs = case["runs"]["runs"].as_array().unwrap().clone();
    assert!(runs.len() >= 11);
    let (out, src) = (l.out.clone(), l.src.clone());
    let roots: [(&Path, &str); 2] = [(&out, "<out>"), (&src, "<src>")];
    let mut written_before = std::collections::BTreeSet::new();
    for run in runs {
        let name = run["name"].as_str().unwrap();
        l.cx.close_dialog("export_failures");
        if name == "nothing_selected" {
            continue; // the window always has its open photo selected
        }
        let mut ids: Vec<u64> = match run.get("ids") {
            Some(ids) => ids.as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect(),
            None => vec![1],
        };
        ids.sort_unstable();
        select_ids(&mut l.cx, &ids);
        let folder = if name == "no_folder" { out.join("nowhere") } else { out.clone() };
        let default_options = json!({"format": "JPEG", "quality": 95, "longest": 0, "color_space": "sRGB", "keep_metadata": false, "naming": "{stem}_grainy", "grayscale": true, "dither": true});
        let options = options_with(run.get("options").unwrap_or(&default_options), &folder);
        commands::start_export(&mut l.cx, options);
        if name == "no_folder" {
            assert!(!l.cx.jobs.running("export"), "nothing starts for a folder that is not there");
            assert!(l.cx.status().contains("저장할 폴더를 선택해 주세요."), "{}", l.cx.status());
            assert!(!folder.exists(), "the folder is not created");
            continue;
        }
        assert!(l.cx.jobs.running("export"), "{name}: the job starts");
        wait(&mut l.cx, "export");
        let list = |dir: &Path| -> std::collections::BTreeSet<String> {
            std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
        };
        let now = list(&out);
        let new_files: Vec<String> = now.difference(&written_before).cloned().collect();
        assert_eq!(
            new_files,
            run["new_files"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect::<Vec<_>>(),
            "{name}: new files"
        );
        written_before = now;
        // The message of the status line and the box with the failures.
        let failures = run["failure_count"].as_u64().unwrap();
        let written = run["outputs"].as_array().unwrap().len();
        let expected = grainy_app::text::tr_with(
            "내보내기 완료: {0}장  ·  실패: {1}장",
            &[&written.to_string(), &failures.to_string()],
        );
        assert_eq!(status_of(&l.cx), expected, "{name}: the message");
        assert_eq!(l.cx.dialog_open("export_failures"), failures > 0, "{name}: the failure box");
        // The saved settings and the history, equal to the oracle's.
        let previous = l.cx.doc.preference(PREFERENCE).map(|v| rel_value(&v, &roots)).unwrap_or(Value::Null);
        assert_eq!(previous, run["previous"], "{name}: previous_photo_export");
        let history = l.cx.doc.preference(HISTORY).map(|v| rel_value(&v, &roots)).unwrap_or(json!([]));
        assert_eq!(history, run["history"], "{name}: export_history");
        // The new records name the files with the oracle's edit signatures.
        let expected = run["outputs"].as_array().unwrap();
        let newest: Vec<&Value> = history.as_array().unwrap().iter().rev().take(expected.len()).rev().collect();
        for (record, output) in newest.iter().zip(expected) {
            assert_eq!(record["path"], output["path"], "{name}");
            assert_eq!(record["photo_id"], output["photo_id"], "{name}");
            assert_eq!(record["edit_signature"], output["edit_signature"], "{name}: edit signature");
        }
    }
    // The sidecars, byte for byte, and the copies of the originals.
    let sidecars = case["runs"]["sidecars"].as_object().unwrap();
    assert!(sidecars.len() >= 3);
    for (name, text) in sidecars {
        let ours = std::fs::read(out.join(name)).unwrap();
        // ElementTree writes the sidecar in text mode: CRLF on Windows (the golden), LF on macOS.
        assert_eq!(String::from_utf8(ours).unwrap(), common::native_lines(text.as_str().unwrap()), "{name}");
    }
    for name in case["runs"]["copies_equal_original"].as_object().unwrap().keys() {
        assert_eq!(std::fs::read(out.join(name)).unwrap(), std::fs::read(src.join(name)).unwrap(), "{name}");
    }
    let files: Vec<String> =
        case["runs"]["files"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_owned()).collect();
    let mut ours: Vec<String> =
        std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    ours.sort();
    assert_eq!(ours, files, "the folder at the end");
}

#[test]
fn the_sizes_and_bytes_are_the_oracles_and_the_command_lines() {
    let _turn = korean();
    let l = library("export-sizes");
    let case = golden();
    let (out, src) = (l.out.clone(), l.src.clone());
    let roots: [(&Path, &str); 2] = [(&out, "<out>"), (&src, "<src>")];
    let records = {
        let catalog = l.cx.doc.metadata_catalog().unwrap();
        let settings: Vec<_> = (1..=4).map(|i| l.cx.doc.settings_of(l.cx.doc.index_of(i).unwrap())).collect();
        export::catalog_records(catalog, &[1, 2, 3, 4], &settings).unwrap()
    };
    let mut checked = 0;
    for run in case["runs"]["runs"].as_array().unwrap() {
        let Some(options) = run.get("options") else { continue };
        if options["format"] == "Original" || run["outputs"].as_array().is_none_or(|o| o.is_empty()) {
            continue;
        }
        let folder = common::temp_dir(&format!("export-sizes-{}", run["name"].as_str().unwrap()));
        let ids: Vec<usize> = run["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize).collect();
        let chosen: Vec<Value> = ids.iter().filter(|i| **i <= 4).map(|i| records[*i - 1].clone()).collect();
        let options = options_with(options, &folder);
        let outcome =
            export::run_records(&chosen, &options, &|_, _| {}, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        for (ours, theirs) in outcome.outputs.iter().zip(run["outputs"].as_array().unwrap()) {
            assert_eq!(ours["size"], theirs["size"], "{}: size of {}", run["name"], theirs["path"]);
            // The same file as a direct engine call (what grainy-cli writes) gives.
            let path = PathBuf::from(ours["path"].as_str().unwrap());
            let id = ours["photo_id"].as_u64().unwrap() as usize;
            let request = options.request().unwrap();
            let direct = folder.join("direct").join(path.file_name().unwrap());
            std::fs::create_dir_all(direct.parent().unwrap()).unwrap();
            let settings = records[id - 1]["settings"].as_object().unwrap();
            grainy_engine::files::export_photo(records[id - 1]["path"].as_str().unwrap(), &direct, settings, &request)
                .unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), std::fs::read(&direct).unwrap(), "{}", path.display());
            checked += 1;
        }
    }
    assert!(checked >= 8, "{checked} files compared");
    let _ = roots;
}

#[test]
fn dither_and_grayscale_do_nothing_for_formats_that_do_not_use_them() {
    // The dialog keeps the flags as ticked and passes them on as the oracle does: the engine ignores them where the
    // oracle's `export_image` does.
    let _turn = korean();
    let l = library("export-flags");
    let settings = [l.cx.doc.settings_of(l.cx.doc.index_of(2).unwrap())];
    let catalog = l.cx.doc.metadata_catalog().unwrap();
    let records = export::catalog_records(catalog, &[2], &settings).unwrap();
    let mut results = Vec::new();
    for (dither, grayscale) in [(true, true), (false, false)] {
        let folder = common::temp_dir(&format!("export-flags-{dither}"));
        let options = ExportOptions {
            folder: folder.to_string_lossy().into_owned(),
            format: FORMATS.iter().position(|f| *f == "PNG 16-bit").unwrap(),
            dither,
            grayscale,
            ..Default::default()
        };
        let outcome =
            export::run_records(&records, &options, &|_, _| {}, &std::sync::atomic::AtomicBool::new(false)).unwrap();
        results.push(std::fs::read(outcome.written()[0].clone()).unwrap());
    }
    assert!(results[0] == results[1], "PNG 16-bit ignores both flags");
}

// ---- the history --------------------------------------------------------------------------------------------

#[test]
fn the_history_dialog_and_the_reset_equal_the_oracles() {
    let _turn = korean();
    let mut l = library("export-history");
    let case = golden();
    let (out, src) = (l.out.clone(), l.src.clone());
    let roots: [(&Path, &str); 2] = [(&out, "<out>"), (&src, "<src>")];
    // The runs of the generator, up to the last one that wrote files.
    for run in case["runs"]["runs"].as_array().unwrap() {
        let name = run["name"].as_str().unwrap();
        if matches!(name, "no_folder" | "nothing_selected") {
            continue;
        }
        let mut ids: Vec<u64> = run["ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap()).collect();
        ids.sort_unstable();
        select_ids(&mut l.cx, &ids);
        commands::start_export(&mut l.cx, options_with(&run["options"], &out));
        wait(&mut l.cx, "export");
    }
    let history = &case["history"];
    let catalog = l.cx.doc.metadata_catalog().unwrap();
    let records = |ids: &[i64]| -> Value {
        let list = grainy_catalog::workflow::export_history(catalog, ids).unwrap();
        rel_value(&Value::Array(list), &roots)
    };
    assert_eq!(records(&[1, 2, 3, 4, 5]), history["all"], "export_history of every photo");
    assert_eq!(records(&[1]), history["one"]);
    assert_eq!(records(&[]), history["none"]);
    // Photo 1 is edited after its export: the records of that photo say so.
    let mut settings = l.cx.doc.stored_settings_of(1).unwrap();
    settings.0.insert("exposure".into(), json!(1.25));
    let catalog = l.cx.doc.metadata_catalog().unwrap();
    catalog.set_settings(1, &settings).unwrap();
    assert_eq!(
        rel_value(&Value::Array(grainy_catalog::workflow::export_history(catalog, &[1, 2, 3, 4, 5]).unwrap()), &roots),
        history["after_edit"]
    );
    // A file is gone: the rows of the dialog.
    for entry in std::fs::read_dir(&out).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "avif") {
            std::fs::remove_file(path).unwrap();
        }
    }
    let catalog = l.cx.doc.metadata_catalog().unwrap();
    for (key, ids) in [("dialog_all", vec![1u64, 2, 3, 4, 5]), ("dialog_one", vec![2]), ("dialog_none", vec![])] {
        let rows = commands::history_rows(catalog, &ids).unwrap();
        let expected = history[key]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), expected.len(), "{key}");
        for (row, golden) in rows.iter().zip(expected) {
            assert_eq!(row.name, golden[0].as_str().unwrap(), "{key}");
            assert_eq!(row.created, golden[1].as_str().unwrap(), "{key}");
            assert_eq!(row.status, golden[2].as_str().unwrap(), "{key}");
            assert_eq!(rel(&row.path, &roots), golden[3].as_str().unwrap(), "{key}");
        }
    }
    // 내보내기 상태 초기화 forgets the selected photos' records only.
    commands::forget(&mut l.cx, &[1, 3]).unwrap();
    let kept = l.cx.doc.preference(HISTORY).map(|v| rel_value(&v, &roots)).unwrap();
    assert_eq!(kept, history["after_reset_1_3"]);
    commands::forget(&mut l.cx, &[]).unwrap();
    let kept = l.cx.doc.preference(HISTORY).map(|v| rel_value(&v, &roots)).unwrap();
    assert_eq!(kept, history["after_reset_none"]);
}

#[test]
fn a_session_exports_without_a_catalogue() {
    let _turn = korean();
    let dir = common::temp_dir("export-session");
    let source = dir.join("garden.png");
    common::save_png(&common::garden(1, [1.0; 3], false), &source);
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let mut session = grainy_app::session::Session::default();
    session.add([source.clone()]);
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(session));
    assert!(commands::saved(&cx).is_none(), "nothing exported yet");
    let options = ExportOptions { folder: out.to_string_lossy().into_owned(), ..Default::default() };
    commands::start_export(&mut cx, options.clone());
    wait(&mut cx, "export");
    assert!(out.join("garden_grainy.jpg").is_file());
    assert_eq!(cx.status(), "내보내기 완료: 1장  ·  실패: 0장");
    // The session keeps what the export used, for 이전 설정으로 내보내기.
    let plan = commands::plan_quick(&mut cx, "previous");
    assert_eq!(plan, Quick::Start(options));
}

#[test]
fn nothing_is_overwritten() {
    let _turn = korean();
    let mut l = library("export-overwrite");
    std::fs::write(l.out.join("a_grainy.jpg"), b"someone else's file").unwrap();
    select_ids(&mut l.cx, &[1]);
    commands::start_export(
        &mut l.cx,
        ExportOptions { folder: l.out.to_string_lossy().into_owned(), ..Default::default() },
    );
    wait(&mut l.cx, "export");
    assert_eq!(std::fs::read(l.out.join("a_grainy.jpg")).unwrap(), b"someone else's file");
    assert!(l.out.join("a_grainy_2.jpg").is_file());
    // An original is never touched either.
    let before = std::fs::read(l.src.join("a.png")).unwrap();
    let options = ExportOptions {
        folder: l.out.to_string_lossy().into_owned(),
        format: FORMATS.iter().position(|f| *f == "Original").unwrap(),
        naming: "{stem}".into(),
        ..Default::default()
    };
    commands::start_export(&mut l.cx, options);
    wait(&mut l.cx, "export");
    assert_eq!(std::fs::read(l.src.join("a.png")).unwrap(), before);
    assert!(l.out.join("a.png").is_file() && l.out.join("a.xmp").is_file());
    let _ = Ordering::Relaxed;
}

// ---- windows ------------------------------------------------------------------------------------------------

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

/// A library of three photos in the Develop view (fixed names; the folders it lives in are never on screen).
fn window(
    language: Language,
    scene: &str,
    setup: impl FnOnce(&mut GrainyApp) + 'static,
) -> Harness<'static, GrainyApp> {
    grainy_i18n::set_language(language);
    let dir = common::temp_dir(scene);
    let source = dir.join("photos");
    std::fs::create_dir_all(&source).unwrap();
    let photos: Vec<PathBuf> = common::eight_photos(&source).into_iter().take(3).collect();
    let library = dir.join("Library");
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let doc = CatalogDocument::open(&library).expect("library");
        let mut app =
            GrainyApp::with_document(&cc.egui_ctx, Box::new(grainy_app::worker::Worker::new()), Box::new(doc));
        app.open_paths(photos);
        app.set_mode(Mode::Develop);
        app
    });
    settle(&mut harness);
    assert_eq!(harness.state().document().len(), 3, "{}", harness.state().status());
    setup(harness.state_mut());
    settle(&mut harness);
    // Which folders of the tree are open depends on when the listing grew: open the drives only.
    common::snapshot::open_folders(harness.state_mut(), &[]);
    settle(&mut harness);
    harness
}

fn previous_export() -> Value {
    json!({"folder": "D:\\Exports\\Garden", "format": "Original", "quality": 90, "longest": 0, "color_space": "sRGB",
           "keep_metadata": false, "naming": "{stem}_{n:03d}", "grayscale": true, "dither": true})
}

fn dialog_snapshot(language: Language, name: &str) {
    let _turn = common::language_lock();
    let mut harness = window(language, name, |app| {
        app.doc.save_preference(PREFERENCE, &previous_export()).unwrap();
        assert!(app.run("file.export"));
    });
    assert!(harness.state().dialog_open("export"));
    common::snapshot::pin_status(harness.state_mut(), "");
    harness.run_steps(2);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_export_dialog_with_original_and_settings_korean() {
    dialog_snapshot(Language::Korean, "export-dialog-original-ko");
}

#[test]
fn the_export_dialog_with_original_and_settings_english() {
    dialog_snapshot(Language::English, "export-dialog-original-en");
}

fn history_snapshot(language: Language, name: &str) {
    let _turn = common::language_lock();
    let mut harness = window(language, name, |app| {
        // Records with fixed paths (no such files: the second row says so) and times.
        let ids: Vec<u64> = app.doc.photos().iter().map(|p| p.id).collect();
        let signature = |text: &str| text.repeat(32);
        let current = grainy_catalog::workflow::edit_signature(&app.doc.stored_settings_of(ids[1]).unwrap()).unwrap();
        let records = json!([
            {"path": "D:\\Exports\\Garden\\garden-01_grainy.jpg", "photo_id": ids[0], "edit_signature": signature("ab"),
             "created": "2026-11-12T09:30:15.250000+00:00"},
            {"path": "D:\\Exports\\Garden\\garden-02_grainy.jpg", "photo_id": ids[1], "edit_signature": current,
             "created": "2026-11-12T09:30:16.500000+00:00"},
            {"path": "D:\\Exports\\Garden\\garden-01.png", "photo_id": ids[0], "edit_signature": signature("ef"),
             "created": "2026-11-13T08:00:00.125000+00:00"},
        ]);
        app.doc.save_preference(HISTORY, &records).unwrap();
        app.doc.toggle_selected(1);
        assert!(app.run("export.history"));
    });
    assert!(harness.state().dialog_open("export_history"));
    common::snapshot::pin_status(harness.state_mut(), "");
    harness.run_steps(2);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_export_history_korean() {
    history_snapshot(Language::Korean, "export-history-ko");
}

#[test]
fn the_export_history_english() {
    history_snapshot(Language::English, "export-history-en");
}

fn failures_snapshot(language: Language, name: &str) {
    let _turn = common::language_lock();
    let mut harness = window(language, name, |app| {
        app.open_dialog(Box::new(grainy_app::dialogs::export::ExportFailures {
            lines: vec![
                "garden-02.png: [Errno 2] No such file or directory".to_owned(),
                "garden-03.tif: the file cannot be read".to_owned(),
            ],
        }));
    });
    assert!(harness.state().dialog_open("export_failures"));
    // The status line shows the message that arrived last: set it once everything has settled.
    common::snapshot::pin_status(
        harness.state_mut(),
        &grainy_app::text::tr_with("내보내기 완료: {0}장  ·  실패: {1}장", &["1", "2"]),
    );
    harness.run_steps(2);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn the_failure_box_korean() {
    failures_snapshot(Language::Korean, "export-failures-ko");
}

#[test]
fn the_failure_box_english() {
    failures_snapshot(Language::English, "export-failures-en");
}
