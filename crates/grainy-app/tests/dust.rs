//! Work package 25: the dust detection dialogs, against the oracle (`golden/app/dust`, made by
//! `tools/golden/gen_app_dust.py` with the Python Grainy's own `DustDialog` and `DustBatchDialog` on a stand-in
//! window). The model tests drive the dialogs without a window (jobs pumped by hand) through the same scripted
//! steps and compare what the dialogs show, the operations they answer and the catalogue rows; the window tests
//! render the real application headless and write `tests/snapshots/dust-*`. Everything works on generated photos
//! copied into temporary folders.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};

use eframe::egui;
use egui_kittest::{Harness, HarnessBuilder};
use grainy_app::dialogs::dust::{self, DustReview};
use grainy_app::dialogs::dust_batch::{self, DustBatch, SessionIds};
use grainy_app::document::catalog::CatalogDocument;
use grainy_app::render::TestCard;
use grainy_app::{AppContext, GrainyApp};
use grainy_catalog::rusqlite::Connection;
use grainy_catalog::{Catalog, Sources};
use grainy_i18n::Language;
use grainy_testkit::Case;
use serde_json::{json, Map, Value};

fn case() -> Value {
    Case::open("app", "dust").json().clone()
}

fn golden_dir() -> PathBuf {
    grainy_testkit::root().join("golden/app/dust")
}

// ---- the library --------------------------------------------------------------------------------------------

const NAMES: [(&str, &str); 4] = [("a", "roll/a.png"), ("b", "roll/b.png"), ("c", "roll/c.png"), ("d", "other/d.png")];

/// The photos copied under `root` with the generator's fixed file time.
fn write_photos(root: &Path, mtime_ns: u64) {
    for (kind, name) in NAMES {
        let path = root.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::copy(golden_dir().join(format!("{kind}.png")), &path).unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(UNIX_EPOCH + Duration::from_nanos(mtime_ns)).unwrap();
    }
}

/// The generator's `build_library`: four photos added with the oracle's metadata, photos 1 and 2 edited once.
fn build(dir: &Path) -> (PathBuf, AppContext) {
    let meta = case();
    let root = dir.join("photos");
    write_photos(&root, meta["mtime_ns"].as_u64().unwrap());
    let stamp = meta["stamp"].as_str().unwrap().to_owned();
    let catalog = Catalog::with_sources(
        dir.join("library"),
        Sources::new(move || stamp.clone(), || "0123456789abcdef0123456789abcdef".to_owned()),
    )
    .unwrap();
    for (_, name) in NAMES {
        let path = root.join(name);
        catalog
            .add(&path.to_string_lossy(), None, Some(&json!({"format": "png", "width": 160, "height": 120})), false)
            .unwrap();
    }
    let mut doc = CatalogDocument::with_catalog(catalog).unwrap();
    doc.refresh_now().unwrap();
    let mut cx = AppContext::new(&egui::Context::default(), Box::new(TestCard::default()), Box::new(doc));
    for (id, changes) in meta["edits"].as_object().unwrap() {
        let id: u64 = id.parse().unwrap();
        open_photo(&mut cx, id);
        for change in changes.as_array().unwrap() {
            cx.doc.edit_mut().set(change[0].as_str().unwrap(), change[1].clone());
        }
        cx.doc.edit_mut().commit("보정 변경");
        cx.doc.flush().unwrap();
    }
    (root, cx)
}

fn open_photo(cx: &mut AppContext, id: u64) {
    let index = cx.doc.index_of(id).expect("photo listed");
    cx.select_photo(index);
    cx.doc.flush().unwrap();
}

/// Runs the finished jobs until none runs (`spawn` answering).
fn pump(cx: &mut AppContext) {
    let started = Instant::now();
    loop {
        for finished in cx.jobs.poll() {
            finished(cx);
        }
        if cx.jobs.jobs_idle() {
            break;
        }
        assert!(started.elapsed() < Duration::from_secs(300), "jobs did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

// ---- what the generator stores ------------------------------------------------------------------------------

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

fn sparse_list(value: &Value) -> Value {
    Value::Array(value.as_array().map(|a| a.iter().map(sparse).collect()).unwrap_or_default())
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

/// Every string with the photos' folder as `$ROOT`.
fn rooted(value: &Value, root: &str) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace(root, "$ROOT")),
        Value::Array(a) => Value::Array(a.iter().map(|v| rooted(v, root)).collect()),
        Value::Object(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), rooted(v, root))).collect()),
        other => other.clone(),
    }
}

/// The generator's `project`: photos, history, preferences and the dust review tables.
fn project(cx: &AppContext, root: &str) -> Value {
    let directory = cx.doc.metadata_catalog().unwrap().directory().to_path_buf();
    let db = Connection::open(directory.join("catalog.sqlite")).unwrap();
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
            let mut shown = Map::new();
            shown.insert("undo".into(), sparse_list(&value["undo"]));
            shown.insert("redo".into(), sparse_list(&value["redo"]));
            for (k, v) in value.as_object().unwrap() {
                if k != "undo" && k != "redo" {
                    shown.insert(k.clone(), v.clone());
                }
            }
            value = Value::Object(shown);
        }
        if key == "undo:dust_batch" {
            for record in value["records"].as_array_mut().unwrap() {
                record["before"] = sparse(&record["before"]);
                record["after"] = sparse(&record["after"]);
                for side in ["undo_before", "undo_after"] {
                    let lists: Map<String, Value> =
                        record[side].as_object().unwrap().iter().map(|(k, v)| (k.clone(), sparse_list(v))).collect();
                    record[side] = Value::Object(lists);
                }
            }
        }
        preferences.insert(key, value);
    }
    let mut statement = db
        .prepare(
            "SELECT id,version,target_key,scope,snapshot,options,created,updated,current_index,revision FROM \
             dust_sessions ORDER BY id",
        )
        .unwrap();
    let sessions: Vec<Value> = statement
        .query_map([], |r| {
            Ok(json!({"id": r.get::<_, String>(0)?, "version": r.get::<_, i64>(1)?,
                      "target_key": r.get::<_, String>(2)?.len(), "scope": r.get::<_, String>(3)?,
                      "snapshot": parse(&r.get::<_, String>(4)?), "options": parse(&r.get::<_, String>(5)?),
                      "created": r.get::<_, String>(6)?, "updated": r.get::<_, String>(7)?,
                      "current_index": r.get::<_, i64>(8)?, "revision": r.get::<_, i64>(9)?}))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let mut statement = db
        .prepare("SELECT session_id,ordinal,photo_id,path,state,payload FROM dust_entries ORDER BY session_id,ordinal")
        .unwrap();
    let entries: Vec<Value> = statement
        .query_map([], |r| {
            let payload: Vec<u8> = r.get(5)?;
            let mut value = parse(&grainy_dust::store::unpack(&payload).unwrap());
            value["record"]["settings"] = sparse(&value["record"]["settings"]);
            if value["result"].is_object() {
                value["result"]["seconds"] = json!(0.0);
            }
            Ok(json!({"session": r.get::<_, String>(0)?, "ordinal": r.get::<_, i64>(1)?,
                      "photo": r.get::<_, i64>(2)?, "path": r.get::<_, String>(3)?, "state": r.get::<_, String>(4)?,
                      "payload": value}))
        })
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rooted(
        &json!({"photos": photos, "history": history, "preferences": preferences, "sessions": sessions,
                "entries": entries}),
        root,
    )
}

/// Compares JSON, numbers within 1e-6, and names the first difference.
fn equal(actual: &Value, expected: &Value, at: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            assert!((a - b).abs() <= 1e-6 * b.abs().max(1.0), "{at}: {a} vs {b}");
        }
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                equal(a.get(k).unwrap_or(&Value::Null), v, &format!("{at}.{k}"));
            }
            let extra: Vec<&String> = a.keys().filter(|k| !b.contains_key(*k)).collect();
            assert!(extra.is_empty(), "{at}: extra keys {extra:?}");
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{at}: lengths\n{actual}\n{expected}");
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                equal(x, y, &format!("{at}[{i}]"));
            }
        }
        _ => assert_eq!(actual, expected, "{at}"),
    }
}

/// The detection time of a status line as the generator's fixed clock gives it.
fn fixed_seconds(text: &str) -> String {
    let marker = "· 검출 ";
    let Some(start) = text.find(marker) else { return text.to_owned() };
    let from = start + marker.len();
    let Some(end) = text[from..].find('초') else { return text.to_owned() };
    format!("{}0.00{}", &text[..from], &text[from + end..])
}

fn dialog_state(d: &DustReview, root: &str) -> Value {
    let list: Vec<Value> = (0..d.spots.len()).map(|i| json!([d.item_text(i), d.checks[i]])).collect();
    let (more, more_text) = d.more();
    let idle = !d.busy;
    rooted(
        &json!({
            "title": d.title(),
            "options": dust::options_json(&d.options()),
            "enabled": {
                "sensitivity": idle, "minimum": idle, "maximum": idle, "polarity": idle, "detailed": idle,
                "soft": d.soft_enabled(), "suppress_grain": d.suppress_enabled(), "reduce_patterns": d.reduce_enabled(),
                "scratches": idle, "scratch_width": d.scratch_width_enabled(), "scan_button": idle,
                "stop_button": d.stop_enabled(), "add_mode": d.manual_enabled(), "stroke_mode": d.manual_enabled(),
                "scale": d.scale_enabled(), "more_button": d.more_enabled(), "apply_button": d.apply_enabled(),
            },
            "list": list,
            "current": d.current,
            "count": d.count_text(),
            "apply": d.apply_text(),
            "more": [more, more_text],
            "status": fixed_seconds(&d.status),
            "scale": d.scale,
            "manual_size": d.manual_size,
            "method": d.repair_method,
            "add_mode": d.add_mode,
            "stroke_mode": d.stroke_mode,
            "view_current": d.view_current,
            "checked": d.selected(),
            "spots": d.spots,
            "pending": d.pending_spots,
            "has_result": d.result.is_some(),
            "preview_note": d.preview_note,
        }),
        root,
    )
}

fn run_step(shared: &dust::Shared, cx: &mut AppContext, step: &Value) {
    let a = step.as_array().unwrap();
    let action = a[0].as_str().unwrap();
    if action == "scan" {
        dust::scan(shared, cx);
        pump(cx);
        return;
    }
    let mut d = shared.borrow_mut();
    match action {
        "toggle" => d.toggle_candidate(a[1].as_u64().unwrap() as usize),
        "row" => d.set_current_row(a[1].as_i64().unwrap()),
        "scale" => d.resize_candidate(a[1].as_i64().unwrap() as i32),
        "all" => d.select_all(a[1].as_bool().unwrap()),
        "add" => d.add_candidate(a[1].as_f64().unwrap(), a[2].as_f64().unwrap()),
        "size" => {
            d.manual_size = a[1].as_i64().unwrap() as i32;
            d.view.stroke_diameter = d.manual_size as f32;
        }
        "stroke" => {
            let points: Vec<[f64; 2]> =
                a[1].as_array().unwrap().iter().map(|p| [p[0].as_f64().unwrap(), p[1].as_f64().unwrap()]).collect();
            d.add_stroke(&points);
        }
        "method" => {
            d.repair_method = a[1].as_str().unwrap().to_owned();
            d.request_preview();
        }
        "sensitivity" => {
            d.sensitivity = a[1].as_i64().unwrap() as i32;
            d.options_changed();
        }
        "scratches" => {
            d.scratches = a[1].as_bool().unwrap();
            d.options_changed();
        }
        "detailed" => {
            d.detailed = a[1].as_bool().unwrap();
            d.options_changed();
        }
        "show_spots" => d.show_spots = a[1].as_bool().unwrap(),
        other => panic!("unknown step {other}"),
    }
}

fn picture_u8(d: &DustReview) -> Vec<u8> {
    let picture = d.picture.as_ref().expect("a picture");
    picture.iter().map(|v| (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8).collect()
}

fn largest_difference(actual: &[u8], expected: &ndarray::ArrayD<u8>) -> u8 {
    assert_eq!(actual.len(), expected.len());
    actual.iter().zip(expected.iter()).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0)
}

#[test]
fn single_photo_review_matches_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let meta = case();
    let golden = &meta["single"];
    let dir = common::temp_dir("dust-single");
    let (root, mut cx) = build(&dir);
    let root_text = root.to_string_lossy().into_owned();
    equal(&project(&cx, &root_text), &meta["single_start"], "start");
    cx.doc.save_preference("dust_detection_options", &meta["preference"]).unwrap();
    open_photo(&mut cx, 1);
    let shared = dust::open(&mut cx, grainy_app::commands::dust::accepted);
    equal(&dialog_state(&shared.borrow(), &root_text), &golden["states"]["initial"], "initial");
    dust::scan(&shared, &mut cx);
    pump(&mut cx);
    equal(&dialog_state(&shared.borrow(), &root_text), &golden["states"]["scanned"], "scanned");
    for (n, step) in golden["steps"].as_array().unwrap().iter().enumerate() {
        run_step(&shared, &mut cx, &step["step"]);
        equal(&dialog_state(&shared.borrow(), &root_text), &step["state"], &format!("step {n} {}", step["step"]));
    }
    // The repair preview and the emphasis view.
    {
        let mut d = shared.borrow_mut();
        d.after = true;
        d.request_preview();
    }
    dust::start_preview(&shared, &mut cx);
    pump(&mut cx);
    let after = picture_u8(&shared.borrow());
    {
        let mut d = shared.borrow_mut();
        d.emphasis = true;
        d.request_preview();
        d.strength = 70;
        d.request_preview();
    }
    dust::start_preview(&shared, &mut cx);
    pump(&mut cx);
    let emphasis = picture_u8(&shared.borrow());
    let golden_after = Case::open("app", "dust").array::<u8, ndarray::IxDyn>("after");
    let golden_emphasis = Case::open("app", "dust").array::<u8, ndarray::IxDyn>("emphasis");
    let (da, de) = (largest_difference(&after, &golden_after), largest_difference(&emphasis, &golden_emphasis));
    println!("preview: largest difference {da}/255 (repair), {de}/255 (emphasis)");
    assert!(da <= 1 && de <= 1, "preview pictures differ: {da} {de}");
    assert_eq!(
        shared.borrow().preview_note,
        grainy_i18n::tr("먼지 강조 · ") + &grainy_i18n::tr("체크된 후보 제거 미리보기")
    );
    // 선택한 먼지 제거.
    shared.borrow_mut().apply_selection(&mut cx);
    {
        let d = shared.borrow();
        assert!(d.accepted);
        equal(&rooted(d.operation.as_ref().unwrap(), &root_text), &golden["operation"], "operation");
        equal(&rooted(d.review_state.as_ref().unwrap(), &root_text), &golden["review"], "review");
        grainy_app::commands::dust::accepted(&mut cx, &d);
    }
    cx.doc.flush().unwrap();
    assert_eq!(cx.status(), grainy_i18n::tr_with("먼지 {0}개를 제거했습니다. 실행 취소로 되돌릴 수 있습니다.", &["6"]));
    equal(&project(&cx, &root_text), &golden["project"], "project");
    // A second dialog: the repaired photo, changed options, a new detection.
    let second = std::rc::Rc::new(std::cell::RefCell::new(DustReview::new(&cx, None)));
    dust::scan(&second, &mut cx);
    pump(&mut cx);
    equal(&dialog_state(&second.borrow(), &root_text), &golden["second"]["scanned"], "second scanned");
    run_step(&second, &mut cx, &json!(["scratches", false]));
    equal(&dialog_state(&second.borrow(), &root_text), &golden["second"]["changed"], "second changed");
    for step in [json!(["sensitivity", 90]), json!(["detailed", true]), json!(["scan"])] {
        run_step(&second, &mut cx, &step);
    }
    equal(&dialog_state(&second.borrow(), &root_text), &golden["second"]["rescanned"], "second rescanned");
    second.borrow_mut().done();
    cx.doc.flush().unwrap();
    equal(&project(&cx, &root_text), &golden["second_project"], "second project");
}

fn batch_state(b: &DustBatch, cx: &AppContext, root: &str) -> Value {
    let names = [
        "work_selector",
        "new_button",
        "forget_button",
        "save_button",
        "scope",
        "sensitivity",
        "minimum",
        "maximum",
        "polarity",
        "detailed",
        "soft",
        "suppress_grain",
        "reduce_patterns",
        "scratches",
        "scratch_width",
        "scan_button",
        "stop_button",
        "retry_button",
        "review_button",
        "next_button",
        "apply_button",
        "undo_button",
        "redo_button",
    ];
    let enabled: Map<String, Value> = names.iter().map(|n| ((*n).to_owned(), json!(b.enabled(cx, n)))).collect();
    let rows: Vec<Value> = (0..b.entries.len()).map(|i| json!(b.row(i))).collect();
    let works: Vec<Value> = b.works.iter().map(|(text, id, _)| json!([undated(text), id])).collect();
    rooted(
        &json!({
            "rows": rows,
            "current": b.current,
            "scope": b.scope,
            "scope_note": b.scope_note,
            "summary": b.summary(),
            "status": b.status,
            "save_note": b.save_note(),
            "scan_button": b.scan_text(),
            "stop_button": b.stop_text(),
            "apply_button": b.apply_text(),
            "enabled": enabled,
            "save_visible": !b.save_error.is_empty(),
            "works": works,
            "work": b.work,
            "session": b.session_id,
            "revision": b.revision,
            "progress": [b.progress.0, b.progress.1, b.progress.2],
            "snapshot": b.snapshot,
            "states": b.entries.iter().map(|e| e["state"].clone()).collect::<Vec<_>>(),
            "options": {"sensitivity": b.sensitivity, "maximum": b.maximum, "scratches": b.scratches},
        }),
        root,
    )
}

/// A saved work's title without its local date and time (`MM/DD HH:MM`): the generator's machine time zone
/// shows in the golden, and the snapshots' frozen clock shows UTC.
fn undated(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let shape: Vec<bool> = "00/00 00:00".chars().map(|c| c == '0').collect();
    let fits = |at: usize| {
        at + shape.len() <= chars.len()
            && shape.iter().enumerate().all(|(i, digit)| {
                let c = chars[at + i];
                if *digit {
                    c.is_ascii_digit()
                } else {
                    c == "00/00 00:00".chars().nth(i).unwrap()
                }
            })
    };
    match (0..chars.len()).find(|at| fits(*at)) {
        Some(at) => chars[..at]
            .iter()
            .chain("MM/DD HH:MM".chars().collect::<Vec<_>>().iter())
            .chain(chars[at + shape.len()..].iter())
            .collect(),
        None => text.to_owned(),
    }
}

/// A scripted review (`scripted_exec`): detect (cached), the steps, 검토 완료; the batch keeps it.
fn review(
    batch: &dust_batch::Shared,
    review: dust::Shared,
    cx: &mut AppContext,
    script: &[Value],
    root: &str,
    golden: &Value,
) {
    let index = batch.borrow().current as usize;
    dust::scan(&review, cx);
    pump(cx);
    equal(&dialog_state(&review.borrow(), root), &golden[0], "review detected");
    for step in script {
        run_step(&review, cx, step);
    }
    review.borrow_mut().apply_selection(cx);
    equal(&dialog_state(&review.borrow(), root), &golden[1], "review accepted");
    let d = review.borrow();
    dust_batch::reviewed(batch, cx, index, &d);
    cx.close_dialog("dust");
}

#[test]
fn batch_review_matches_the_oracle() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let meta = case();
    // The golden's only backslashes are the separators of `$ROOT\roll\a.png`: on macOS the oracle writes `/`.
    let golden = common::native_json(&meta["batch"]).as_array().unwrap().clone();
    let dir = common::temp_dir("dust-batch");
    let (root, mut cx) = build(&dir);
    let root_text = root.to_string_lossy().into_owned();
    equal(&project(&cx, &root_text), &common::native_json(&meta["batch_start"]), "start");
    for n in 1..=3 {
        cx.state::<SessionIds>().queue.push_back(format!("{n:032x}"));
    }
    open_photo(&mut cx, 1);
    let three = cx.doc.index_of(3).unwrap();
    cx.doc.toggle_selected(three);
    let at = std::cell::Cell::new(0usize);
    let check = |cx: &mut AppContext, b: &dust_batch::Shared, name: &str| {
        let entry = &golden[at.get()];
        assert_eq!(entry["name"], name);
        let mut expected = entry["state"].clone();
        for work in expected["works"].as_array_mut().into_iter().flatten() {
            work[0] = json!(undated(work[0].as_str().unwrap_or_default()));
        }
        equal(&batch_state(&b.borrow(), cx, &root_text), &expected, name);
        if let Some(last) = entry["messages"].as_array().and_then(|m| m.last()) {
            assert_eq!(cx.status(), last.as_str().unwrap(), "{name}: status line");
        }
        at.set(at.get() + 1);
    };
    let b = dust_batch::open(&mut cx).unwrap();
    pump(&mut cx);
    check(&mut cx, &b, "opened");
    b.borrow_mut().scope_changed(&mut cx, "folder");
    check(&mut cx, &b, "folder_scope");
    dust_batch::start(&b, &mut cx, None);
    pump(&mut cx);
    check(&mut cx, &b, "detected");
    let tables = |cx: &AppContext, name: &str| {
        let index = at.get();
        assert_eq!(golden[index]["name"], name);
        equal(&project(cx, &root.to_string_lossy()), &golden[index]["project"], name);
        at.set(index + 1);
    };
    tables(&cx, "detected_tables");
    b.borrow_mut().set_current(0);
    let r = dust_batch::review_selected(&b, &mut cx).expect("a review opens");
    let script = [json!(["toggle", 0]), json!(["add", 0.75, 0.25]), json!(["method", "texture"])];
    review(&b, r, &mut cx, &script, &root_text, &golden[at.get()]["review"]);
    check(&mut cx, &b, "reviewed_first");
    let r = dust_batch::review_next(&b, &mut cx).expect("the next review opens");
    review(&b, r, &mut cx, &[], &root_text, &golden[at.get()]["review"]);
    check(&mut cx, &b, "reviewed_next");
    tables(&cx, "reviewed_tables");
    dust_batch::apply(&b, &mut cx);
    check(&mut cx, &b, "applied");
    tables(&cx, "applied_tables");
    dust_batch::restore_batch(&b, &mut cx, false);
    check(&mut cx, &b, "undone");
    tables(&cx, "undone_tables");
    dust_batch::restore_batch(&b, &mut cx, true);
    check(&mut cx, &b, "redone");
    tables(&cx, "redone_tables");
    assert!(dust_batch::close(&b, &mut cx));
    tables(&cx, "closed_tables");
    // Opened again on the folder: the saved work.
    let mut query = cx.doc.query();
    query.folder = Some(root.join("roll").to_string_lossy().into_owned());
    cx.doc.set_query(query).unwrap();
    let b = dust_batch::open(&mut cx).unwrap();
    pump(&mut cx);
    check(&mut cx, &b, "reopened");
    // Photo 3 changed outside the dialog.
    {
        let catalog = cx.doc.metadata_catalog().unwrap();
        let mut settings = grainy_catalog::settings(&catalog.photo(3).unwrap().unwrap()["settings"]).unwrap();
        settings.set("exposure", 0.5);
        catalog.edit(3, &settings, "바깥 변경").unwrap();
    }
    b.borrow_mut().set_current(0);
    assert!(dust_batch::review_next(&b, &mut cx).is_none());
    check(&mut cx, &b, "changed_source");
    dust_batch::retry(&b, &mut cx);
    pump(&mut cx);
    check(&mut cx, &b, "retried");
    dust_batch::start(&b, &mut cx, None);
    pump(&mut cx);
    check(&mut cx, &b, "detected_again");
    tables(&cx, "detected_again_tables");
    b.borrow_mut().forget_confirmed(&mut cx);
    check(&mut cx, &b, "forgotten");
    tables(&cx, "forgotten_tables");
    assert_eq!(at.get(), golden.len());
}

// ---- unit checks of the dialog's own rules ------------------------------------------------------------------

#[test]
fn python_formatting() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    assert_eq!(dust::py_g(5.6), "5.6");
    assert_eq!(dust::py_g(12.0), "12");
    assert_eq!(dust::py_g(7.123456789), "7.12346");
    assert_eq!(dust::py_g(1234567.0), "1.23457e+06");
    assert_eq!(dust::py_g(0.00001), "1e-05");
    assert_eq!(dust::thousands(5000), "5,000");
    assert_eq!(dust::thousands(999), "999");
    let options = grainy_dust::Options { minimum: 50, maximum: 20, ..Default::default() };
    assert_eq!(dust::validate(&options).unwrap_err(), "먼지 크기는 2–160 px 범위에서 최소 ≤ 최대여야 합니다.");
    let saved = json!({"sensitivity": 70, "unknown": 1, "polarity": "dark"});
    let o = dust::initial_options(Some(&saved));
    assert_eq!((o.sensitivity, o.polarity.as_str()), (70, "dark"));
    let o = dust::initial_options(Some(&json!({"sensitivity": 0})));
    assert_eq!(o.sensitivity, 50, "invalid saved options give the defaults");
}

#[test]
fn paging_cancel_and_limits() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("dust-paging");
    let (_root, mut cx) = build(&dir);
    open_photo(&mut cx, 1);
    let shared = dust::open(&mut cx, |_, _| {});
    dust::scan(&shared, &mut cx);
    pump(&mut cx);
    let mut d = shared.borrow_mut();
    // 후보 더 보기: pending candidates join 500 at a time, unchecked when they need a check.
    let template = d.spots[0].clone();
    d.pending_spots = (0..1200)
        .map(|i| {
            let mut s = template.clone();
            s["review"] = json!(i % 2 == 0);
            s
        })
        .collect();
    let listed = d.spots.len();
    assert_eq!(d.more(), (true, grainy_i18n::tr_with("후보 {0}개 더 보기 · 남은 {1}개", &["500", "1,200"])));
    d.show_more();
    assert_eq!(d.spots.len(), listed + 500);
    assert!(!d.checks[listed] && d.checks[listed + 1], "review candidates start unchecked");
    d.show_more();
    d.show_more();
    assert_eq!(d.pending_spots.len(), 0);
    assert!(!d.more().0);
    // The manual limit (MAX_CANDIDATES + 1000).
    d.pending_spots = vec![template.clone(); 6000 - d.spots.len()];
    d.add_candidate(0.5, 0.5);
    assert_eq!(d.status, grainy_i18n::tr("후보가 많습니다. 일부를 먼저 적용한 뒤 다시 검출하세요."));
    d.pending_spots.clear();
    // Cancel: the detection stops and the photo is untouched.
    drop(d);
    shared.borrow_mut().sensitivity = 51;
    shared.borrow_mut().options_changed();
    dust::scan(&shared, &mut cx);
    shared.borrow_mut().cancel_scan();
    assert!(!shared.borrow().stop_enabled());
    pump(&mut cx);
    let d = shared.borrow();
    assert!(!d.busy);
    assert!(
        d.status == grainy_i18n::tr("검출을 취소했습니다. 사진은 변경되지 않았습니다.") || d.result.is_some(),
        "{}",
        d.status
    );
}

#[test]
fn hit_test_prefers_the_nearest_candidate() {
    let _turn = common::language_lock();
    grainy_i18n::set_language(Language::Korean);
    let dir = common::temp_dir("dust-hit");
    let (_root, mut cx) = build(&dir);
    open_photo(&mut cx, 1);
    let shared = dust::open(&mut cx, |_, _| {});
    dust::scan(&shared, &mut cx);
    pump(&mut cx);
    dust::start_preview(&shared, &mut cx);
    pump(&mut cx);
    let d = shared.borrow();
    let (w, h) = d.scene_size();
    for (i, spot) in d.spots.iter().enumerate() {
        let x = spot["x"].as_f64().unwrap() as f32 * (w - 1.0);
        let y = spot["y"].as_f64().unwrap() as f32 * (h - 1.0);
        assert_eq!(d.hit(egui::pos2(x, y)), Some(i));
    }
    assert_eq!(d.hit(egui::pos2(-50.0, -50.0)), None);
}

// ---- snapshots ----------------------------------------------------------------------------------------------

fn builder() -> HarnessBuilder<GrainyApp> {
    grainy_app::context::FROZEN_TIMING.store(true, std::sync::atomic::Ordering::Relaxed);
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

/// A library window with the four photos, photo 1 open in the Develop view's Healing tool.
fn library_harness(name: &str) -> Harness<'static, GrainyApp> {
    let dir = common::temp_dir(name);
    let root = dir.join("photos");
    write_photos(&root, case()["mtime_ns"].as_u64().unwrap());
    let library = dir.join("library");
    let paths: Vec<PathBuf> = NAMES.iter().map(|(_, n)| root.join(n)).collect();
    let stamp = case()["stamp"].as_str().unwrap().to_owned();
    let mut harness = builder().wgpu().build_eframe(move |cc| {
        let sources = Sources::new(move || stamp.clone(), || "0123456789abcdef0123456789abcdef".to_owned());
        let doc = CatalogDocument::with_catalog(Catalog::with_sources(&library, sources).unwrap()).expect("library");
        let mut app =
            GrainyApp::with_document(&cc.egui_ctx, Box::new(grainy_app::worker::Worker::new()), Box::new(doc));
        app.open_paths(paths);
        app
    });
    settle(&mut harness);
    harness.state_mut().select_photo(0);
    settle(&mut harness);
    harness
}

fn dust_review_snapshot(language: Language, name: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let mut harness = library_harness(name);
    assert!(harness.state_mut().run("heal.detect_dust"));
    settle(&mut harness);
    // Fixed detection time; the first candidate chosen, a manual spot added, the repair preview on.
    {
        let shared = dust::last_opened().expect("a review is open");
        let mut d = shared.borrow_mut();
        if let Some(result) = d.result.as_mut() {
            result["seconds"] = json!(0.25);
        }
        d.result_status();
        d.toggle_candidate(1);
        d.add_candidate(0.25, 0.5);
        d.after = true;
        d.request_preview();
    }
    settle(&mut harness);
    std::thread::sleep(Duration::from_millis(150));
    common::snapshot::open_folders(harness.state_mut(), &["roll", "other"]);
    common::snapshot::pin_status(harness.state_mut(), "");
    settle(&mut harness);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn dust_review_korean() {
    dust_review_snapshot(Language::Korean, "dust-review-ko");
}

#[test]
fn dust_review_english() {
    dust_review_snapshot(Language::English, "dust-review-en");
}

fn dust_batch_snapshot(language: Language, name: &str) {
    let _turn = common::language_lock();
    grainy_i18n::set_language(language);
    let mut harness = library_harness(name);
    // Three photos selected (no folder path on screen: the scope is the selection).
    for index in [1, 2] {
        harness.state_mut().document_mut().toggle_selected(index);
    }
    harness.state_mut().state::<SessionIds>().queue.push_back(format!("{:032x}", 1));
    assert!(harness.state_mut().run("heal.detect_dust_batch"));
    settle(&mut harness);
    {
        let batch = dust_batch::last_opened().expect("the batch window is open");
        assert_eq!(batch.borrow().scope, "selection");
        dust_batch::start(&batch, harness.state_mut(), None);
    }
    settle(&mut harness);
    common::snapshot::open_folders(harness.state_mut(), &["roll", "other"]);
    common::snapshot::pin_status(harness.state_mut(), "");
    settle(&mut harness);
    save(&mut harness, name);
    grainy_i18n::set_language(Language::Korean);
}

#[test]
fn dust_batch_korean() {
    dust_batch_snapshot(Language::Korean, "dust-batch-ko");
}

#[test]
fn dust_batch_english() {
    dust_batch_snapshot(Language::English, "dust-batch-en");
}
